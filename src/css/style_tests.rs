use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::css::{PseudoElement, parse_style_attribute, parse_stylesheet};
use crate::dom::{NodeHandle, ShadowRootMode};

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
fn shadow_stylesheets_respect_tree_scope_host_and_slotted_boundaries() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let host = NodeHandle::element("x-card");
    host.set_attribute("class", "active");
    let light = NodeHandle::element("span");
    light.set_attribute("id", "chosen");
    light.set_attribute("class", "item");
    host.append_child(light.clone());
    document.append_child(body.clone());
    body.append_child(host.clone());

    let root = host.attach_shadow(ShadowRootMode::Closed).unwrap();
    let inside = NodeHandle::element("span");
    inside.set_attribute("class", "inside");
    let slot = NodeHandle::element("slot");
    slot.set_attribute("class", "special");
    root.append_child(inside.clone());
    root.append_child(slot);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "x-card { color: purple; } .inside { width: 99px; } \
             .item { margin-left: 44px; padding-left: 6px !important; }",
        )
        .unwrap(),
    );
    resolver.add_scoped_stylesheet(
        Origin::Author,
        parse_stylesheet(
            ".inside { width: 11px; } slot { color: orange; } \
             :host(.active) { height: 22px; } :host { height: 23px; } \
             :host(.active) .inside { min-width: 12px; } \
             :host(.active) > .inside { max-width: 13px; } \
             :host::before { width: 14px; } \
             ::slotted(.item) { margin-left: 33px; padding-left: 5px !important; } \
             slot.special::slotted(.item) { border-left-width: 7px; } \
             ::slotted(#chosen) { outline-width: 8px; } ::slotted(*) { outline-width: 9px; }",
        )
        .unwrap(),
        root,
    );

    assert_eq!(
        resolver.computed_style(&inside).get("width"),
        Some(&ComputedValue::Px(11.0))
    );
    assert_eq!(
        resolver.computed_style(&inside).get("color"),
        Some(&ComputedValue::Color("purple".to_string())),
        "shadow children inherit from the host"
    );
    assert_eq!(
        resolver.computed_style(&inside).get("min-width"),
        Some(&ComputedValue::Px(12.0)),
        ":host() can condition a descendant selector"
    );
    assert_eq!(
        resolver.computed_style(&inside).get("max-width"),
        Some(&ComputedValue::Px(13.0)),
        ":host() participates in child combinator matching"
    );
    assert_eq!(
        resolver
            .computed_pseudo_style(&host, PseudoElement::Before)
            .unwrap()
            .get("width"),
        Some(&ComputedValue::Px(14.0)),
        ":host can target a host pseudo-element"
    );
    assert_eq!(
        resolver.computed_style(&host).get("height"),
        Some(&ComputedValue::Px(22.0))
    );
    let light_style = resolver.computed_style(&light);
    assert_eq!(
        light_style.get("margin-left"),
        Some(&ComputedValue::Px(44.0)),
        "normal declarations from the outer tree win"
    );
    assert_eq!(
        light_style.get("padding-left"),
        Some(&ComputedValue::Px(5.0)),
        "important declarations from the inner tree win"
    );
    assert_eq!(
        light_style.get("color"),
        Some(&ComputedValue::Color("orange".to_string())),
        "assigned elements inherit from their slot"
    );
    assert_eq!(
        light_style.get("border-left-width"),
        Some(&ComputedValue::Px(7.0)),
        "the selector before ::slotted() matches the assigned slot"
    );
    assert_eq!(
        light_style.get("outline-width"),
        Some(&ComputedValue::Px(8.0)),
        "the ::slotted() argument contributes specificity"
    );
}

#[test]
fn shadow_host_encapsulation_order_reverses_for_important_rules() {
    fn host_color(document_important: bool, shadow_important: bool) -> ComputedStyle {
        let document = NodeHandle::document();
        let host = NodeHandle::element("x-card");
        document.append_child(host.clone());
        let root = host.attach_shadow(ShadowRootMode::Open).unwrap();
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(&format!(
                "x-card {{ color: green{}; }}",
                if document_important { " !important" } else { "" }
            ))
            .unwrap(),
        );
        resolver.add_scoped_stylesheet(
            Origin::Author,
            parse_stylesheet(&format!(
                ":host {{ color: red{}; }}",
                if shadow_important { " !important" } else { "" }
            ))
            .unwrap(),
            root,
        );
        resolver.computed_style(&host)
    }

    assert_eq!(
        host_color(false, false).get("color"),
        Some(&ComputedValue::Color("green".to_string()))
    );
    assert_eq!(
        host_color(true, true).get("color"),
        Some(&ComputedValue::Color("red".to_string()))
    );
}

#[test]
fn document_part_rules_match_only_exposed_names_on_the_selected_host() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    document.append_child(body.clone());

    let selected_host = NodeHandle::element("x-card");
    selected_host.set_attribute("class", "selected");
    body.append_child(selected_host.clone());
    let selected_root = selected_host.attach_shadow(ShadowRootMode::Closed).unwrap();
    let exposed = NodeHandle::element("span");
    exposed.set_attribute("part", "label accent");
    exposed.set_attribute("class", "private-class");
    let private = NodeHandle::element("span");
    private.set_attribute("class", "private-class");
    selected_root.append_child(exposed.clone());
    selected_root.append_child(private.clone());

    let other_host = NodeHandle::element("x-card");
    body.append_child(other_host.clone());
    let other_root = other_host.attach_shadow(ShadowRootMode::Open).unwrap();
    let other = NodeHandle::element("span");
    other.set_attribute("part", "label");
    other_root.append_child(other.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            ".private-class { width: 99px; } \
             x-card.selected::part(label) { width: 11px; } \
             x-card.selected::part(accent) { height: 12px; }",
        )
        .unwrap(),
    );
    resolver.add_scoped_stylesheet(
        Origin::Author,
        parse_stylesheet(":host::part(label) { min-height: 13px; }").unwrap(),
        selected_root,
    );

    assert_eq!(
        resolver.computed_style(&exposed).get("width"),
        Some(&ComputedValue::Px(11.0))
    );
    assert_eq!(
        resolver.computed_style(&exposed).get("height"),
        Some(&ComputedValue::Px(12.0)),
        "every token in the part attribute exposes the element"
    );
    assert_eq!(
        resolver.computed_style(&exposed).get("min-height"),
        Some(&ComputedValue::Px(13.0)),
        ":host::part() exposes parts to rules in the owning shadow root"
    );
    assert_eq!(resolver.computed_style(&private).get("width"), None);
    assert_eq!(
        resolver.computed_style(&other).get("width"),
        None,
        "the selector prefix must match the element's own host"
    );
}

#[test]
fn nested_exportparts_forwards_same_name_renames_and_ignores_invalid_entries() {
    let document = NodeHandle::document();
    let outer_host = NodeHandle::element("x-outer");
    document.append_child(outer_host.clone());
    let outer_root = outer_host.attach_shadow(ShadowRootMode::Open).unwrap();
    let middle_host = NodeHandle::element("x-middle");
    middle_host.set_attribute(
        "exportparts",
        "direct, inner: renamed, broken:, :also-broken, extra words, inner: second",
    );
    outer_root.append_child(middle_host.clone());
    let middle_root = middle_host.attach_shadow(ShadowRootMode::Closed).unwrap();
    let inner_host = NodeHandle::element("x-inner");
    inner_host.set_attribute("exportparts", "seed: direct, seed: inner, hidden");
    middle_root.append_child(inner_host.clone());
    let inner_root = inner_host.attach_shadow(ShadowRootMode::Open).unwrap();
    let leaf = NodeHandle::element("span");
    leaf.set_attribute("part", "seed hidden");
    inner_root.append_child(leaf.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "x-outer::part(direct) { width: 1px; } \
             x-outer::part(renamed) { height: 2px; } \
             x-outer::part(second) { min-width: 3px; } \
             x-outer::part(inner) { max-width: 4px; } \
             x-outer::part(hidden) { margin-left: 5px; }",
        )
        .unwrap(),
    );
    let style = resolver.computed_style(&leaf);
    assert_eq!(style.get("width"), Some(&ComputedValue::Px(1.0)));
    assert_eq!(style.get("height"), Some(&ComputedValue::Px(2.0)));
    assert_eq!(style.get("min-width"), Some(&ComputedValue::Px(3.0)));
    assert_eq!(style.get("max-width"), None, "renaming does not retain the inner name");
    assert_eq!(style.get("margin-left"), None, "unexported names do not cross the nested root");
}

#[test]
fn part_cascade_uses_shadow_encapsulation_order() {
    let document = NodeHandle::document();
    let host = NodeHandle::element("x-card");
    document.append_child(host.clone());
    let root = host.attach_shadow(ShadowRootMode::Open).unwrap();
    let label = NodeHandle::element("span");
    label.set_attribute("class", "label");
    label.set_attribute("part", "label");
    label.set_attribute(
        "style",
        "width: 15px; height: 15px !important; min-width: 15px !important",
    );
    root.append_child(label.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "x-card::part(label) { width: 20px; height: 20px !important; min-width: 20px !important; }",
        )
        .unwrap(),
    );
    resolver.add_scoped_stylesheet(
        Origin::Author,
        parse_stylesheet(
            ".label { width: 10px; height: 10px; min-width: 10px !important; }",
        )
        .unwrap(),
        root,
    );
    let style = resolver.computed_style(&label);
    assert_eq!(
        style.get("width"),
        Some(&ComputedValue::Px(20.0)),
        "outer normal rules win over shadow and inline normal declarations"
    );
    assert_eq!(
        style.get("height"),
        Some(&ComputedValue::Px(15.0)),
        "inner inline important declarations win over outer important rules"
    );
    assert_eq!(
        style.get("min-width"),
        Some(&ComputedValue::Px(15.0)),
        "inline wins within the same inner tree scope"
    );

    let bare_document = NodeHandle::document();
    let bare_host = NodeHandle::element("x-bare");
    bare_document.append_child(bare_host.clone());
    let bare_root = bare_host.attach_shadow(ShadowRootMode::Closed).unwrap();
    let bare_part = NodeHandle::element("span");
    bare_part.set_attribute("part", "label");
    bare_part.set_attribute("style", "width: 5px; height: 5px !important");
    bare_root.append_child(bare_part.clone());
    let mut bare_resolver = StyleResolver::new();
    bare_resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "x-bare::part(label) { width: 6px; height: 6px !important; }",
        )
        .unwrap(),
    );
    let bare_style = bare_resolver.computed_style(&bare_part);
    assert_eq!(bare_style.get("width"), Some(&ComputedValue::Px(6.0)));
    assert_eq!(
        bare_style.get("height"),
        Some(&ComputedValue::Px(5.0)),
        "inline declarations retain their inner context without a shadow stylesheet"
    );
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
fn scope_roots_limits_and_scope_pseudo_control_the_cascade() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let card = NodeHandle::element("section");
    card.set_attribute("class", "card");
    let direct = NodeHandle::element("p");
    direct.set_attribute("id", "direct");
    let stop = NodeHandle::element("div");
    stop.set_attribute("class", "stop");
    let limited = NodeHandle::element("p");
    limited.set_attribute("id", "limited");
    let outside = NodeHandle::element("p");
    outside.set_attribute("id", "outside");

    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(card.clone());
    card.append_child(direct.clone());
    card.append_child(stop.clone());
    stop.append_child(limited.clone());
    body.append_child(outside.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "p { color: black; width: 1px; height: 2px; }\
             @scope (.card) to (.stop) {\
               p { color: red; height: 20px; }\
               :scope > p { width: 10px; }\
             }",
        )
        .unwrap(),
    );

    let direct_style = resolver.computed_style(&direct);
    assert_eq!(
        direct_style.get("color"),
        Some(&ComputedValue::Color("red".to_string()))
    );
    assert_eq!(direct_style.get("width"), Some(&ComputedValue::Px(10.0)));
    assert_eq!(direct_style.get("height"), Some(&ComputedValue::Px(20.0)));

    let limited_style = resolver.computed_style(&limited);
    assert_eq!(
        limited_style.get("color"),
        Some(&ComputedValue::Color("black".to_string()))
    );
    assert_eq!(limited_style.get("height"), Some(&ComputedValue::Px(2.0)));
    assert_eq!(
        resolver.computed_style(&outside).get("color"),
        Some(&ComputedValue::Color("black".to_string()))
    );
}

#[test]
fn scope_proximity_wins_after_specificity_and_before_source_order() {
    let document = NodeHandle::document();
    let outer = NodeHandle::element("section");
    outer.set_attribute("class", "outer");
    let inner = NodeHandle::element("div");
    inner.set_attribute("class", "inner");
    let target = NodeHandle::element("p");
    target.set_attribute("id", "target");
    target.set_attribute("class", "specific");
    document.append_child(outer.clone());
    outer.append_child(inner.clone());
    inner.append_child(target.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "@scope (.inner) { p { color: blue; width: 20px; } }\
             @scope (.inner) { > p { margin-left: 7px; } }\
             @scope (.outer) { p { color: red; width: 30px; } }\
             @scope (.outer) { #target { color: green; } }\
             .specific { width: 40px; }",
        )
        .unwrap(),
    );

    let style = resolver.computed_style(&target);
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("green".to_string())),
        "higher selector specificity wins before scope proximity"
    );
    assert_eq!(
        style.get("width"),
        Some(&ComputedValue::Px(40.0)),
        "the scope prelude does not add specificity"
    );
    assert_eq!(style.get("margin-left"), Some(&ComputedValue::Px(7.0)));

    let mut proximity_only = StyleResolver::new();
    proximity_only.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "@scope (.inner) { p { color: blue; } }\
             @scope (.outer) { p { color: red; } }",
        )
        .unwrap(),
    );
    assert_eq!(
        proximity_only.computed_style(&target).get("color"),
        Some(&ComputedValue::Color("blue".to_string())),
        "the closer root wins before the later source order"
    );
}

#[test]
fn scope_root_requires_scope_pseudo_but_can_match_in_an_outer_scope() {
    let document = NodeHandle::document();
    let outer = NodeHandle::element("section");
    outer.set_attribute("class", "card");
    let inner = NodeHandle::element("section");
    inner.set_attribute("class", "card");
    document.append_child(outer.clone());
    outer.append_child(inner.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "@scope (.card) {\
               .card { height: 8px; }\
               :scope { width: 9px; }\
             }",
        )
        .unwrap(),
    );

    let outer_style = resolver.computed_style(&outer);
    assert_eq!(outer_style.get("height"), None);
    assert_eq!(outer_style.get("width"), Some(&ComputedValue::Px(9.0)));

    let inner_style = resolver.computed_style(&inner);
    assert_eq!(
        inner_style.get("height"),
        Some(&ComputedValue::Px(8.0)),
        "the nested root is still a descendant of the outer matching root"
    );
    assert_eq!(inner_style.get("width"), Some(&ComputedValue::Px(9.0)));
}

#[test]
fn scoped_selector_ancestor_matching_stops_at_the_scope_root() {
    let document = NodeHandle::document();
    let outside = NodeHandle::element("div");
    outside.set_attribute("class", "outside");
    let root = NodeHandle::element("section");
    root.set_attribute("class", "root");
    let target = NodeHandle::element("p");
    document.append_child(outside.clone());
    outside.append_child(root.clone());
    root.append_child(target.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "p { color: black; } @scope (.root) { .outside p { color: red; } }",
        )
        .unwrap(),
    );
    assert_eq!(
        resolver.computed_style(&target).get("color"),
        Some(&ComputedValue::Color("black".to_string()))
    );
}

#[test]
fn scope_limits_can_reference_ancestors_outside_the_scope_root() {
    let document = NodeHandle::document();
    let sidebar = NodeHandle::element("aside");
    sidebar.set_attribute("class", "sidebar");
    let root = NodeHandle::element("section");
    root.set_attribute("class", "feature");
    let direct = NodeHandle::element("p");
    let limit = NodeHandle::element("div");
    limit.set_attribute("class", "limit");
    let nested = NodeHandle::element("p");
    document.append_child(sidebar.clone());
    sidebar.append_child(root.clone());
    root.append_child(direct.clone());
    root.append_child(limit.clone());
    limit.append_child(nested.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "p { color: black; } @scope (.feature) to (.sidebar :scope .limit) { p { color: red; } }",
        )
        .unwrap(),
    );

    assert_eq!(
        resolver.computed_style(&direct).get("color"),
        Some(&ComputedValue::Color("red".to_string()))
    );
    assert_eq!(
        resolver.computed_style(&nested).get("color"),
        Some(&ComputedValue::Color("black".to_string())),
        "a scope limit may use :scope to match an ancestor outside the root"
    );
}

#[test]
fn nested_scope_start_is_relative_to_the_outer_scope() {
    let document = NodeHandle::document();
    let sidebar = NodeHandle::element("aside");
    sidebar.set_attribute("class", "sidebar");
    let outer = NodeHandle::element("section");
    outer.set_attribute("class", "outer");
    let inner = NodeHandle::element("section");
    inner.set_attribute("class", "inner");
    let target = NodeHandle::element("p");
    document.append_child(sidebar.clone());
    sidebar.append_child(outer.clone());
    outer.append_child(inner.clone());
    inner.append_child(target.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "p { color: black; } @scope (.outer) { @scope (.sidebar :scope .inner) { p { color: red; } } }",
        )
        .unwrap(),
    );

    assert_eq!(
        resolver.computed_style(&target).get("color"),
        Some(&ComputedValue::Color("red".to_string()))
    );
}

#[test]
fn scope_reference_detection_descends_into_has_arguments() {
    let stylesheet = parse_stylesheet(":has(:scope > .child) { color: red; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    assert!(selector_references_scope(&rule.selectors[0]));
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
fn important_origin_order_matches_css_cascade() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();

    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: red !important; }").unwrap(),
    );
    resolver.add_stylesheet(
        Origin::User,
        parse_stylesheet("h1 { color: green !important; }").unwrap(),
    );
    resolver.add_stylesheet(
        Origin::UserAgent,
        parse_stylesheet("h1 { color: blue !important; }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    assert_eq!(style.get("color"), Some(&ComputedValue::Color("blue".to_string())));
}

#[test]
fn inline_style_beats_author_specificity() {
    let (_document, _body, title, _html) = sample_tree();
    title.set_attribute("style", "color: green");
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("#hero { color: red; }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    assert_eq!(style.get("color"), Some(&ComputedValue::Color("green".to_string())));
}

#[test]
fn author_important_beats_normal_inline_style() {
    let (_document, _body, title, _html) = sample_tree();
    title.set_attribute("style", "color: green");
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: red !important; }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    assert_eq!(style.get("color"), Some(&ComputedValue::Color("red".to_string())));
}

#[test]
fn inline_important_beats_author_important() {
    let (_document, _body, title, _html) = sample_tree();
    title.set_attribute("style", "color: green !important");
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("#hero { color: red !important; }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    assert_eq!(style.get("color"), Some(&ComputedValue::Color("green".to_string())));
}

#[test]
fn user_important_beats_inline_important() {
    let (_document, _body, title, _html) = sample_tree();
    title.set_attribute("style", "color: green !important");
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::User,
        parse_stylesheet("h1 { color: purple !important; }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    assert_eq!(style.get("color"), Some(&ComputedValue::Color("purple".to_string())));
}

#[test]
fn inline_width_beats_presentational_width_hint() {
    let element = NodeHandle::element("div");
    element.set_attribute("width", "50");
    element.set_attribute("style", "width: 100px");
    let mut resolver = StyleResolver::new();

    let style = resolver.computed_style(&element);
    assert_eq!(style.get("width"), Some(&ComputedValue::Px(100.0)));
}

#[test]
fn inline_style_does_not_apply_to_pseudo_elements() {
    let (_document, _body, title, _html) = sample_tree();
    title.set_attribute("style", "color: green");
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1::before { content: \"prefix\"; color: red; }").unwrap(),
    );

    let style = resolver
        .computed_pseudo_style(&title, PseudoElement::Before)
        .unwrap();
    assert_eq!(style.get("color"), Some(&ComputedValue::Color("red".to_string())));
}

#[test]
fn inline_style_uses_forgiving_declaration_parser() {
    let element = NodeHandle::element("div");
    element.set_attribute(
        "style",
        "background: url(data:image/png;base64,AAA); color red; width: 10px",
    );
    let mut resolver = StyleResolver::new();

    let style = resolver.computed_style(&element);
    assert_eq!(
        style.get("background-image"),
        Some(&ComputedValue::Keyword(
            "url(data:image/png;base64,AAA)".to_string()
        ))
    );
    assert_eq!(style.get("width"), Some(&ComputedValue::Px(10.0)));
}

#[test]
fn inline_style_canonicalizes_webkit_properties() {
    let element = NodeHandle::element("div");
    element.set_attribute("style", "-webkit-transform: translateX(10px)");
    let mut resolver = StyleResolver::new();

    let style = resolver.computed_style(&element);
    assert_eq!(
        style.get("transform"),
        Some(&ComputedValue::Keyword("translateX(10px)".to_string()))
    );
    assert_eq!(style.get("-webkit-transform"), None);
}

#[test]
fn iframe_inline_width_remains_supported() {
    let iframe = NodeHandle::element("iframe");
    iframe.set_attribute("style", "width: 200px");
    let mut resolver = StyleResolver::new();

    let style = resolver.computed_style(&iframe);
    assert_eq!(style.get("width"), Some(&ComputedValue::Px(200.0)));
}

#[test]
fn inline_declarations_follow_priority_and_source_order() {
    let element = NodeHandle::element("div");
    element.set_attribute("style", "width: 10px; width: 20px");
    let style = StyleResolver::new().computed_style(&element);
    assert_eq!(style.get("width"), Some(&ComputedValue::Px(20.0)));

    let element = NodeHandle::element("div");
    element.set_attribute("style", "color: blue !important; color: red");
    let style = StyleResolver::new().computed_style(&element);
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("blue".to_string()))
    );
}

#[test]
fn inline_custom_properties_resolve_in_inline_values() {
    let element = NodeHandle::element("div");
    element.set_attribute("style", "--x: 5px; width: var(--x)");
    let style = StyleResolver::new().computed_style(&element);
    assert_eq!(style.get("width"), Some(&ComputedValue::Px(5.0)));
}

#[test]
fn inline_property_names_are_ascii_case_insensitive() {
    let element = NodeHandle::element("div");
    element.set_attribute("style", "COLOR: red");
    let style = StyleResolver::new().computed_style(&element);
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("red".to_string()))
    );
}

#[test]
fn position_accepts_sticky_and_discards_invalid_keywords() {
    let element = NodeHandle::element("div");
    element.set_attribute("style", "position: sticky; position: sideways");
    let style = StyleResolver::new().computed_style(&element);
    assert_eq!(style.get("position"), Some(&ComputedValue::Keyword("sticky".to_string())));

    let plain = NodeHandle::element("div");
    assert_eq!(
        StyleResolver::new().computed_style(&plain).get("position"),
        Some(&ComputedValue::Keyword("static".to_string()))
    );
}

#[test]
fn identifies_supported_property_names() {
    assert!(is_supported_property("background-color"));
    assert!(is_supported_property("position"));
    assert!(is_supported_property("transform"));
    assert!(is_supported_property("filter"));
    assert!(is_supported_property("backdrop-filter"));

    for property in [
        "transform-origin",
        "animation",
        "animation-delay",
        "animation-direction",
        "animation-duration",
        "animation-iteration-count",
        "animation-play-state",
        "animation-timing-function",
        "margin-inline-start",
        "margin-inline-end",
        "margin-block-start",
        "margin-block-end",
        "padding-inline-start",
        "padding-inline-end",
        "padding-block-start",
        "padding-block-end",
    ] {
        assert!(
            is_supported_property(property),
            "expected `{property}` to be registered as a supported property"
        );
    }

    // `animation` shorthand (e.g. `animation: fade 0.3s forwards`) を解決すると
    // `expand_animation_shorthand` が longhand へ展開しつつ元の `animation` 宣言も
    // 再 emit する。ここで候補になる宣言名一式が supported であることを確認し、
    // shorthand 使用ページで未対応ログが出ないことを担保する。
    for property in [
        "animation",
        "animation-name",
        "animation-fill-mode",
        "animation-duration",
    ] {
        assert!(
            is_supported_property(property),
            "expected animation shorthand candidate `{property}` to be supported"
        );
    }
}

#[test]
fn css_supports_uses_parser_and_supported_property_table() {
    assert!(supports_declaration("display", "block"));
    assert!(supports_declaration("margin", "10px 20px"));
    assert!(supports_declaration("cursor", "pointer"));
    assert!(supports_declaration("filter", "brightness(150%) blur(2px)"));
    assert!(supports_declaration("backdrop-filter", "grayscale(1)"));
    assert!(supports_declaration("FILTER", "blur(2px)"));
    assert!(supports_declaration("--theme-color", "rgb(1, 2, 3)"));

    assert!(!supports_declaration("future-property", "value"));
    assert!(!supports_declaration("width", "12"));
    assert!(!supports_declaration("cursor", "definitely-not-a-cursor"));
    assert!(!supports_declaration("filter", "blur(-1px)"));
    assert!(!supports_declaration("color", "red; width: 10px"));
    assert!(!supports_declaration("display", ""));
}

#[test]
fn expands_grid_placement_shorthands_and_keeps_longhands() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "h1 { grid-column: 1 / span 3; grid-row: span 2; grid-row-end: 4; }",
        )
        .unwrap(),
    );
    let style = resolver.computed_style(&title);
    assert_eq!(style.get("grid-column-start"), Some(&ComputedValue::Keyword("1".to_string())));
    assert_eq!(style.get("grid-column-end"), Some(&ComputedValue::Keyword("span 3".to_string())));
    assert_eq!(style.get("grid-row-start"), Some(&ComputedValue::Keyword("span 2".to_string())));
    assert_eq!(style.get("grid-row-end"), Some(&ComputedValue::Keyword("4".to_string())));
    for property in ["grid-column", "grid-column-start", "grid-column-end", "grid-row", "grid-row-start", "grid-row-end"] {
        assert!(is_supported_property(property));
    }
}

#[test]
fn expands_grid_area_shorthand_with_named_and_numeric_lines() {
    let stylesheet = parse_stylesheet(
        ".named { grid-area: title; } .lines { grid-area: 1 / 2 / 4; }",
    )
    .unwrap();

    let Rule::Style(named) = &stylesheet.rules[0] else {
        panic!("expected named style rule");
    };
    let named_values: Vec<_> = named
        .declarations
        .iter()
        .map(|declaration| (declaration.name.as_str(), &declaration.value))
        .collect();
    assert_eq!(
        named_values,
        vec![
            ("grid-row-start", &Value::Keyword("title".to_string())),
            ("grid-column-start", &Value::Keyword("title".to_string())),
            ("grid-row-end", &Value::Keyword("title".to_string())),
            ("grid-column-end", &Value::Keyword("title".to_string())),
        ]
    );

    let Rule::Style(lines) = &stylesheet.rules[1] else {
        panic!("expected line style rule");
    };
    let line_values: Vec<_> = lines
        .declarations
        .iter()
        .map(|declaration| (declaration.name.as_str(), &declaration.value))
        .collect();
    assert_eq!(
        line_values,
        vec![
            ("grid-row-start", &Value::Number(1.0)),
            ("grid-column-start", &Value::Number(2.0)),
            ("grid-row-end", &Value::Number(4.0)),
            ("grid-column-end", &Value::Keyword("auto".to_string())),
        ]
    );
}

#[test]
fn expands_grid_template_track_and_area_forms() {
    let stylesheet = parse_stylesheet(
        ".tracks { grid-template: 30px 40px / 100px 1fr; } \
         .areas { grid-template: \"hero hero\" 60px \"nav main\" auto / calc(10vw + 20px) 1fr; }",
    )
    .unwrap();

    let Rule::Style(tracks) = &stylesheet.rules[0] else {
        panic!("expected track style rule");
    };
    assert_eq!(tracks.declarations.len(), 2);
    assert_eq!(tracks.declarations[0].name, "grid-template-rows");
    assert_eq!(tracks.declarations[1].name, "grid-template-columns");

    let Rule::Style(areas) = &stylesheet.rules[1] else {
        panic!("expected area style rule");
    };
    assert_eq!(areas.declarations.len(), 3);
    assert_eq!(areas.declarations[0].name, "grid-template-areas");
    assert_eq!(
        areas.declarations[0].value,
        Value::List(vec![
            Value::String("hero hero".to_string()),
            Value::String("nav main".to_string()),
        ])
    );
    assert_eq!(areas.declarations[1].name, "grid-template-rows");
    assert_eq!(areas.declarations[2].name, "grid-template-columns");
}

#[test]
fn preserves_grid_template_area_row_boundaries_in_computed_style() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { grid-template-areas: \"header header\" \"side main\"; }")
            .unwrap(),
    );

    let style = resolver.computed_style(&title);
    assert_eq!(
        style.get("grid-template-areas"),
        Some(&ComputedValue::Keyword(
            "\"header header\" \"side main\"".to_string()
        ))
    );
    for property in ["grid-template-areas", "grid-area", "grid-template"] {
        assert!(is_supported_property(property));
    }
}

#[test]
fn expands_grid_alignment_shorthands() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "h1 { place-items: end center; place-self: center; place-content: center; }",
        )
        .unwrap(),
    );
    let style = resolver.computed_style(&title);
    assert_eq!(style.get("align-items"), Some(&ComputedValue::Keyword("end".to_string())));
    assert_eq!(style.get("justify-items"), Some(&ComputedValue::Keyword("center".to_string())));
    assert_eq!(style.get("align-self"), Some(&ComputedValue::Keyword("center".to_string())));
    assert_eq!(style.get("justify-self"), Some(&ComputedValue::Keyword("center".to_string())));
    assert_eq!(style.get("align-content"), Some(&ComputedValue::Keyword("center".to_string())));
    assert_eq!(style.get("justify-content"), Some(&ComputedValue::Keyword("center".to_string())));
}

#[test]
fn canonicalizes_prefixed_flex_properties_to_standard_names() {
    let aliases = [
        ("-webkit-align-items", "align-items", "center"),
        ("-ms-flex-align", "align-items", "center"),
        ("-webkit-box-align", "align-items", "center"),
        ("-webkit-justify-content", "justify-content", "center"),
        ("-ms-flex-pack", "justify-content", "center"),
        ("-webkit-box-pack", "justify-content", "center"),
        ("-webkit-flex-shrink", "flex-shrink", "2"),
        ("-ms-flex-negative", "flex-shrink", "2"),
        ("-webkit-flex-grow", "flex-grow", "2"),
        ("-webkit-flex-direction", "flex-direction", "column"),
        ("-webkit-flex-wrap", "flex-wrap", "wrap"),
    ];

    for (alias, standard, value) in aliases {
        let element = NodeHandle::element("div");
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(&format!("div {{ {alias}: {value}; }}")).unwrap(),
        );

        let style = resolver.computed_style(&element);
        assert!(style.get(standard).is_some(), "{alias} should map to {standard}");
        assert_eq!(style.get(alias), None, "{alias} should not remain in computed style");
    }
}

#[test]
fn standard_flex_property_overrides_prefixed_fallback() {
    let element = NodeHandle::element("div");
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "div { -webkit-align-items: start; align-items: end; \
                    -ms-flex-pack: start; justify-content: space-between; \
                    -webkit-flex-shrink: 2; flex-shrink: 1; }",
        )
        .unwrap(),
    );

    let style = resolver.computed_style(&element);
    assert_eq!(style.get("align-items"), Some(&ComputedValue::Keyword("end".to_string())));
    assert_eq!(
        style.get("justify-content"),
        Some(&ComputedValue::Keyword("space-between".to_string()))
    );
    assert_eq!(style.get("flex-shrink"), Some(&ComputedValue::Number(1.0)));

    let reverse = NodeHandle::element("div");
    let mut reverse_resolver = StyleResolver::new();
    reverse_resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "div { align-items: end; -webkit-align-items: start; \
                    justify-content: space-between; -ms-flex-pack: start; \
                    flex-shrink: 1; -webkit-flex-shrink: 2; }",
        )
        .unwrap(),
    );

    let reverse_style = reverse_resolver.computed_style(&reverse);
    assert_eq!(
        reverse_style.get("align-items"),
        Some(&ComputedValue::Keyword("end".to_string()))
    );
    assert_eq!(
        reverse_style.get("justify-content"),
        Some(&ComputedValue::Keyword("space-between".to_string()))
    );
    assert_eq!(
        reverse_style.get("flex-shrink"),
        Some(&ComputedValue::Number(1.0))
    );
}

#[test]
fn keyframe_standard_property_overrides_prefixed_alias_in_either_order() {
    let document = NodeHandle::document();
    let div = NodeHandle::element("div");
    document.append_child(div.clone());
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "@keyframes align { to { align-items: end; -webkit-align-items: start; } } \
             div { animation-name: align; animation-fill-mode: forwards; }",
        )
        .unwrap(),
    );

    let style = resolver.computed_style(&div);
    assert_eq!(
        style.get("align-items"),
        Some(&ComputedValue::Keyword("end".to_string()))
    );
}

#[test]
fn applies_legacy_html_presentational_hints() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let cell = NodeHandle::element("td");
    cell.set_attribute("bgcolor", "336699");
    cell.set_attribute("align", "center");
    cell.set_attribute("width", "50%");
    cell.set_attribute("height", "24px");
    cell.set_attribute("face", "Hiragino Sans, sans-serif");
    body.set_attribute("text", "#112233");
    body.set_attribute("background", "legacy/wallpaper.png");
    body.set_attribute("width", "640");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(cell.clone());

    let mut resolver = StyleResolver::new();
    let body_style = resolver.computed_style(&body);
    let cell_style = resolver.computed_style(&cell);

    assert_eq!(
        body_style.get("color"),
        Some(&ComputedValue::Color("#112233".to_string()))
    );
    assert_eq!(
        body_style.get("background-image"),
        Some(&ComputedValue::Keyword(
            "url(\"legacy/wallpaper.png\")".to_string()
        ))
    );
    assert_eq!(body_style.get("width"), Some(&ComputedValue::Px(640.0)));
    assert_eq!(
        cell_style.get("background-color"),
        Some(&ComputedValue::Color("#336699".to_string()))
    );
    assert_eq!(
        cell_style.get("text-align"),
        Some(&ComputedValue::Keyword("center".to_string()))
    );
    assert_eq!(
        cell_style.get("width"),
        Some(&ComputedValue::Percentage(50.0))
    );
    assert_eq!(cell_style.get("height"), Some(&ComputedValue::Px(24.0)));
    assert_eq!(
        cell_style.get("font-family"),
        Some(&ComputedValue::Keyword(
            "Hiragino Sans, sans-serif".to_string()
        ))
    );
}

#[test]
fn ignores_invalid_legacy_dimension_hints() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let cell = NodeHandle::element("td");
    cell.set_attribute("width", "abc");
    cell.set_attribute("height", "");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(cell.clone());

    let mut resolver = StyleResolver::new();
    let cell_style = resolver.computed_style(&cell);

    assert!(!cell_style.properties().contains_key("width"));
    assert!(!cell_style.properties().contains_key("height"));
}

#[test]
fn keeps_comma_separated_font_family_value() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { font-family: Arial, sans-serif; }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    assert_eq!(
        style.get("font-family"),
        Some(&ComputedValue::Keyword("Arial, sans-serif".to_string()))
    );
}

#[test]
fn keeps_transform_list_values_in_computed_style() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { transform: translateX(10px) translateY(6px); }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    let value = match style.get("transform") {
        Some(ComputedValue::Keyword(value)) => value.to_ascii_lowercase(),
        other => panic!("unexpected transform value: {other:?}"),
    };
    assert!(value.contains("translatex(10px)"));
    assert!(value.contains("translatey(6px)"));
}

#[test]
fn computes_transform_origin_and_initial_transform_values() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { transform-origin: right 25%; }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    assert_eq!(
        style.get("transform"),
        Some(&ComputedValue::Keyword("none".to_string()))
    );
    assert_eq!(
        style.get("transform-origin"),
        Some(&ComputedValue::Keyword("right 25%".to_string()))
    );
}

#[test]
fn transition_shorthand_expands_lists_and_computes_initial_values() {
    let element = NodeHandle::element("div");
    element.set_attribute(
        "style",
        "transition: opacity 200ms linear 50ms, transform 1s ease-in;",
    );
    let mut resolver = StyleResolver::new();
    let style = resolver.computed_style(&element);

    assert_eq!(
        style.get("transition-property"),
        Some(&ComputedValue::Keyword("opacity, transform".to_string()))
    );
    assert_eq!(
        style.get("transition-duration"),
        Some(&ComputedValue::Keyword("0.2s, 1s".to_string()))
    );
    assert_eq!(
        style.get("transition-timing-function"),
        Some(&ComputedValue::Keyword("linear, ease-in".to_string()))
    );
    assert_eq!(
        style.get("transition-delay"),
        Some(&ComputedValue::Keyword("0.05s, 0s".to_string()))
    );
    assert_eq!(
        style.get("transition"),
        Some(&ComputedValue::Keyword(
            "opacity 0.2s linear 0.05s, transform 1s ease-in".to_string()
        ))
    );

    let plain = NodeHandle::element("span");
    let initial = resolver.computed_style(&plain);
    assert_eq!(
        initial.get("transition-property"),
        Some(&ComputedValue::Keyword("all".to_string()))
    );
    assert_eq!(
        initial.get("transition-duration"),
        Some(&ComputedValue::Keyword("0s".to_string()))
    );
    assert_eq!(
        initial.get("transition-timing-function"),
        Some(&ComputedValue::Keyword("ease".to_string()))
    );
    assert_eq!(
        initial.get("transition-delay"),
        Some(&ComputedValue::Keyword("0s".to_string()))
    );
    assert_eq!(
        initial.get("transition"),
        Some(&ComputedValue::Keyword("all".to_string()))
    );
}

#[test]
fn transition_property_preserves_unknown_custom_ident_case() {
    let element = NodeHandle::element("div");
    element.set_attribute(
        "style",
        "transition-property: ALL, INVALID, SYNTAX, SRC, WIDTH;",
    );
    let mut resolver = StyleResolver::new();
    assert_eq!(
        resolver
            .computed_style(&element)
            .get("transition-property"),
        Some(&ComputedValue::Keyword(
            "all, INVALID, SYNTAX, SRC, width".to_string()
        ))
    );
}

#[test]
fn invalid_transition_declaration_does_not_override_valid_value() {
    let element = NodeHandle::element("div");
    element.set_attribute(
        "style",
        "transition-duration: 200ms; transition-duration: -1s; \
         transition-timing-function: linear; transition-timing-function: cubic-bezier(2, 0, 0, 1);",
    );
    let mut resolver = StyleResolver::new();
    let style = resolver.computed_style(&element);

    assert_eq!(
        style.get("transition-duration"),
        Some(&ComputedValue::Keyword("0.2s".to_string()))
    );
    assert_eq!(
        style.get("transition-timing-function"),
        Some(&ComputedValue::Keyword("linear".to_string()))
    );
    assert!(supports_declaration(
        "transition",
        "opacity 200ms ease-in 50ms"
    ));
    assert!(!supports_declaration("transition-duration", "-1s"));
}

#[test]
fn transition_timeline_samples_number_and_length_intermediate_values() {
    let element = NodeHandle::element("div");
    element.set_attribute(
        "style",
        "opacity: 0; width: 10px; transition: opacity 1s linear, width 2s linear;",
    );
    let mut resolver = StyleResolver::new();
    let initial = resolver.computed_style(&element);
    assert_eq!(initial.get("opacity"), Some(&ComputedValue::Number(0.0)));
    assert_eq!(initial.get("width"), Some(&ComputedValue::Px(10.0)));

    element.set_attribute(
        "style",
        "opacity: 1; width: 30px; transition: opacity 1s linear, width 2s linear;",
    );
    resolver.invalidate_style_cache_for_test();
    let start = resolver.computed_style(&element);
    assert_eq!(start.get("opacity"), Some(&ComputedValue::Number(0.0)));
    assert_eq!(start.get("width"), Some(&ComputedValue::Px(10.0)));

    resolver.set_transition_time_ms(500.0);
    let middle = resolver.computed_style(&element);
    assert_eq!(middle.get("opacity"), Some(&ComputedValue::Number(0.5)));
    assert_eq!(middle.get("width"), Some(&ComputedValue::Px(15.0)));

    resolver.set_transition_time_ms(2_000.0);
    let end = resolver.computed_style(&element);
    assert_eq!(end.get("opacity"), Some(&ComputedValue::Number(1.0)));
    assert_eq!(end.get("width"), Some(&ComputedValue::Px(30.0)));
}

#[test]
fn transition_uses_last_matching_property_and_repeats_shorter_lists() {
    let element = NodeHandle::element("div");
    element.set_attribute(
        "style",
        "opacity: 0; transition-property: all, opacity; transition-duration: 10s, 1s; transition-timing-function: linear;",
    );
    let mut resolver = StyleResolver::new();
    let _ = resolver.computed_style(&element);

    element.set_attribute(
        "style",
        "opacity: 1; transition-property: all, opacity; transition-duration: 10s, 1s; transition-timing-function: linear;",
    );
    resolver.invalidate_style_cache_for_test();
    let _ = resolver.computed_style(&element);
    resolver.set_transition_time_ms(500.0);

    assert_eq!(
        resolver.computed_style(&element).get("opacity"),
        Some(&ComputedValue::Number(0.5))
    );
}

#[test]
fn transition_interpolates_color_with_premultiplied_alpha() {
    let element = NodeHandle::element("div");
    element.set_attribute(
        "style",
        "background-color: transparent; transition: background-color 1s linear;",
    );
    let mut resolver = StyleResolver::new();
    let _ = resolver.computed_style(&element);

    element.set_attribute(
        "style",
        "background-color: rgb(255, 0, 0); transition: background-color 1s linear;",
    );
    resolver.invalidate_style_cache_for_test();
    let _ = resolver.computed_style(&element);
    resolver.set_transition_time_ms(500.0);

    assert_eq!(
        resolver.computed_style(&element).get("background-color"),
        Some(&ComputedValue::Color("rgba(255, 0, 0, 0.5)".to_string()))
    );
}

#[test]
fn reversing_transition_shortens_from_the_current_value() {
    let element = NodeHandle::element("div");
    element.set_attribute("style", "opacity: 0; transition: opacity 1s linear;");
    let mut resolver = StyleResolver::new();
    let _ = resolver.computed_style(&element);

    element.set_attribute("style", "opacity: 1; transition: opacity 1s linear;");
    resolver.invalidate_style_cache_for_test();
    let _ = resolver.computed_style(&element);
    resolver.set_transition_time_ms(500.0);
    assert_eq!(
        resolver.computed_style(&element).get("opacity"),
        Some(&ComputedValue::Number(0.5))
    );

    element.set_attribute("style", "opacity: 0; transition: opacity 1s linear;");
    resolver.invalidate_style_cache_for_test();
    assert_eq!(
        resolver.computed_style(&element).get("opacity"),
        Some(&ComputedValue::Number(0.5))
    );
    resolver.set_transition_time_ms(750.0);
    assert_eq!(
        resolver.computed_style(&element).get("opacity"),
        Some(&ComputedValue::Number(0.25))
    );
    resolver.set_transition_time_ms(1_000.0);
    assert_eq!(
        resolver.computed_style(&element).get("opacity"),
        Some(&ComputedValue::Number(0.0))
    );
}

#[test]
fn negative_transition_delay_starts_from_the_advanced_value() {
    let element = NodeHandle::element("div");
    element.set_attribute(
        "style",
        "opacity: 0; transition: opacity 1s linear -500ms;",
    );
    let mut resolver = StyleResolver::new();
    let _ = resolver.computed_style(&element);

    element.set_attribute(
        "style",
        "opacity: 1; transition: opacity 1s linear -500ms;",
    );
    resolver.invalidate_style_cache_for_test();
    assert_eq!(
        resolver.computed_style(&element).get("opacity"),
        Some(&ComputedValue::Number(0.5))
    );
}

#[test]
fn transition_all_does_not_interpolate_discrete_number_properties() {
    let element = NodeHandle::element("div");
    element.set_attribute("style", "z-index: 1; transition: all 1s linear;");
    let mut resolver = StyleResolver::new();
    let _ = resolver.computed_style(&element);

    element.set_attribute("style", "z-index: 3; transition: all 1s linear;");
    resolver.invalidate_style_cache_for_test();
    let changed = resolver.computed_style(&element);
    assert_eq!(changed.get("z-index"), Some(&ComputedValue::Number(3.0)));
    resolver.set_transition_time_ms(500.0);
    assert_eq!(
        resolver.computed_style(&element).get("z-index"),
        Some(&ComputedValue::Number(3.0))
    );
}

#[test]
fn invalid_transform_does_not_override_an_earlier_valid_declaration() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { transform: rotate(45deg); transform: scale(2, nope); }")
            .unwrap(),
    );

    assert_eq!(
        resolver.computed_style(&title).get("transform"),
        Some(&ComputedValue::Keyword("rotate(45deg)".to_string()))
    );
    assert!(!supports_declaration("transform", "translateX(10px) bogus(1)"));
    assert!(supports_declaration(
        "transform",
        "translate(50%, 2em) rotate(.5turn) scale(2)"
    ));
}

#[test]
fn expands_two_value_gap_shorthand_into_row_and_column_gap() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { gap: 10px 20px; }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    assert_eq!(style.get("row-gap"), Some(&ComputedValue::Px(10.0)));
    assert_eq!(style.get("column-gap"), Some(&ComputedValue::Px(20.0)));
    assert_eq!(style.get("gap"), None);
}

#[test]
fn sqlite_logging_creates_schema_and_accumulates_occurrences() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let db_path = std::env::temp_dir().join(format!("omoikane-unsupported-css-{unique}.db"));
    let db_path_str = db_path.to_string_lossy().to_string();

    persist_unsupported_css_to_sqlite(&db_path_str, "transform", "translateX(10px)");
    persist_unsupported_css_to_sqlite(&db_path_str, "transform", "translateX(10px)");
    persist_unsupported_css_to_sqlite(&db_path_str, "filter", "blur(4px)");

    let conn = Connection::open(&db_path_str).unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT property, value, occurrences
             FROM unsupported_css_log
             ORDER BY property, value",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0],
        ("filter".to_string(), "blur(4px)".to_string(), 1_i64)
    );
    assert_eq!(
        rows[1],
        (
            "transform".to_string(),
            "translateX(10px)".to_string(),
            2_i64
        )
    );

    drop(stmt);
    drop(conn);
    close_sqlite_connection_for_path(&db_path_str);
    let _ = fs::remove_file(db_path);
}

#[test]
fn sqlite_top_n_query_orders_by_occurrences() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let db_path = std::env::temp_dir().join(format!("omoikane-unsupported-css-topn-{unique}.db"));
    let db_path_str = db_path.to_string_lossy().to_string();

    persist_unsupported_css_to_sqlite(&db_path_str, "transform", "translateX(10px)");
    persist_unsupported_css_to_sqlite(&db_path_str, "transform", "translateX(10px)");
    persist_unsupported_css_to_sqlite(&db_path_str, "filter", "blur(4px)");
    persist_unsupported_css_to_sqlite(&db_path_str, "backdrop-filter", "blur(4px)");
    persist_unsupported_css_to_sqlite(&db_path_str, "backdrop-filter", "blur(4px)");
    persist_unsupported_css_to_sqlite(&db_path_str, "backdrop-filter", "blur(4px)");

    let conn = Connection::open(&db_path_str).unwrap();
    let rows = query_unsupported_css_top_n(&conn, 2).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, "backdrop-filter");
    assert_eq!(rows[0].2, 3);
    assert_eq!(rows[1].0, "transform");
    assert_eq!(rows[1].2, 2);

    drop(conn);
    close_sqlite_connection_for_path(&db_path_str);
    let _ = fs::remove_file(db_path);
}

#[test]
fn sqlite_audit_separates_vendor_prefixed_properties() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let db_path = std::env::temp_dir().join(format!("omoikane-css-category-{unique}.db"));
    let db_path_str = db_path.to_string_lossy().to_string();
    persist_css_audit_to_sqlite(
        &db_path_str,
        CssAuditCategory::VendorPrefixed,
        "-moz-user-select",
        "none",
    );
    persist_css_audit_to_sqlite(
        &db_path_str,
        CssAuditCategory::Unsupported,
        "future-layout",
        "enabled",
    );
    let conn = Connection::open(&db_path_str).unwrap();
    let mut stmt = conn
        .prepare("SELECT property, category FROM unsupported_css_log ORDER BY property")
        .unwrap();
    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            ("-moz-user-select".to_string(), "vendor-prefixed".to_string()),
            ("future-layout".to_string(), "unsupported".to_string()),
        ]
    );
    drop(stmt);
    drop(conn);
    close_sqlite_connection_for_path(&db_path_str);
    fs::remove_file(db_path).unwrap();
}

#[test]
fn sanitizes_url_like_values_in_unsupported_css_logging() {
    let value = "url(\"https://example.com/a?x=1\") blur(4px) data:image/png;base64,AAAABBBB";
    let sanitized = sanitize_unsupported_css_log_value(value);
    assert!(!sanitized.contains("example.com"));
    assert!(!sanitized.contains("data:image"));
    assert!(!sanitized.contains("AAAABBBB"));
    assert!(sanitized.contains("[redacted-url]"));
}

#[test]
fn ignores_custom_properties_for_unsupported_logging() {
    assert!(should_ignore_unsupported_css_logging("--brand-color"));
    assert!(!should_ignore_unsupported_css_logging("transform"));
}

#[test]
fn audit_classifies_expanded_border_shorthands_and_vendor_prefixes() {
    for property in ["border-width", "border-style", "border-color"] {
        assert!(is_supported_property(property), "{property} should be supported");
        assert_eq!(css_audit_category(property), None);
    }
    let declarations = parse_style_attribute("border: 1px solid red");
    assert!(!declarations.is_empty());
    assert!(
        declarations
            .iter()
            .all(|declaration| is_supported_property(&declaration.name))
    );
    assert_eq!(
        css_audit_category("-moz-user-select"),
        Some(CssAuditCategory::VendorPrefixed)
    );
    assert_eq!(
        css_audit_category("future-layout"),
        Some(CssAuditCategory::Unsupported)
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
    // h1 UA default: 2em = 40px (parent body 20px * 2)
    assert_eq!(title_style.get("font-size"), Some(&ComputedValue::Px(40.0)));
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
    // CSS 2.1 §4.3.2: em unit uses the element's own computed font-size
    assert_eq!(style.get("margin-top"), Some(&ComputedValue::Px(60.0)));
}

#[test]
fn resolves_clamp_font_size_with_viewport_units() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.set_viewport(1280.0, 900.0);
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { font-size: clamp(26px, 6.5vw, 38px); }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    assert_eq!(style.get("font-size"), Some(&ComputedValue::Px(38.0)));
}

#[test]
fn resolves_clamp_font_size_with_calc_percentage() {
    let (_document, body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { font-size: 20px; } h1 { font-size: clamp(100%, calc(150%), 200%); }",
        )
        .unwrap(),
    );

    let _ = resolver.computed_style(&body);
    let style = resolver.computed_style(&title);
    assert_eq!(style.get("font-size"), Some(&ComputedValue::Px(30.0)));
}

#[test]
fn clamp_uses_minimum_when_it_exceeds_maximum() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { font-size: clamp(40px, 30px, 20px); }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    assert_eq!(style.get("font-size"), Some(&ComputedValue::Px(40.0)));
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
    // h1 UA default: 2em = 32px (parent 16px * 2)
    assert_eq!(style.get("font-size"), Some(&ComputedValue::Px(32.0)));
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
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("black".to_string()))
    );
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

#[test]
fn resolves_explicit_inherit_keyword_from_parent() {
    let (_document, body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("body { float: right; } h1 { float: inherit; }").unwrap(),
    );

    let body_style = resolver.computed_style(&body);
    let title_style = resolver.computed_style(&title);

    assert_eq!(
        body_style.get("float"),
        Some(&ComputedValue::Keyword("right".to_string()))
    );
    assert_eq!(
        title_style.get("float"),
        Some(&ComputedValue::Keyword("right".to_string()))
    );
}

#[test]
fn border_style_none_zeroes_side_width_even_when_width_only_comes_from_shorthand() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { border: solid 12px transparent; border-style: none solid; }")
            .unwrap(),
    );

    let style = resolver.computed_style(&title);

    assert_eq!(
        style.get("border-top-style"),
        Some(&ComputedValue::Keyword("none".to_string()))
    );
    assert_eq!(
        style.get("border-bottom-style"),
        Some(&ComputedValue::Keyword("none".to_string()))
    );
    assert_eq!(style.get("border-top-width"), Some(&ComputedValue::Px(0.0)));
    assert_eq!(
        style.get("border-bottom-width"),
        Some(&ComputedValue::Px(0.0))
    );
    assert_eq!(
        style.get("border-right-style"),
        Some(&ComputedValue::Keyword("solid".to_string()))
    );
    assert_eq!(
        style.get("border-left-style"),
        Some(&ComputedValue::Keyword("solid".to_string()))
    );
}

#[test]
fn resolves_var_from_inherited_root_custom_properties() {
    let (_document, body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            ":root { --theme: rgb(255, 255, 255); --primary: #123456; } \
             body { background-color: var(--theme); color: var(--primary); }",
        )
        .unwrap(),
    );

    let body_style = resolver.computed_style(&body);
    let title_style = resolver.computed_style(&title);

    assert_eq!(
        body_style.get("background-color"),
        Some(&ComputedValue::Color("#ffffff".to_string()))
    );
    assert_eq!(
        body_style.get("color"),
        Some(&ComputedValue::Color("#123456".to_string()))
    );
    assert_eq!(
        title_style.get("color"),
        Some(&ComputedValue::Color("#123456".to_string()))
    );
}

#[test]
fn resolves_var_with_fallback_for_missing_custom_property() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: var(--missing-color, blue); }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("blue".to_string()))
    );
}

#[test]
fn drops_declaration_when_var_cannot_be_resolved() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: var(--missing-color); }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("black".to_string()))
    );
}

#[test]
fn resolves_calc_with_var_lengths() {
    let (_document, body, _title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            ":root { --main-width: 720px; --gap: 24px; } \
             body { max-width: calc(var(--main-width) + var(--gap) * 2); }",
        )
        .unwrap(),
    );

    let style = resolver.computed_style(&body);
    assert_eq!(style.get("max-width"), Some(&ComputedValue::Px(768.0)));
}

#[test]
fn resolves_calc_with_var_lengths_without_operator_whitespace() {
    let (_document, body, _title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            ":root { --main-width: 720px; --gap: 24px; } \
             body { max-width: calc(var(--main-width)+var(--gap)*2); }",
        )
        .unwrap(),
    );

    let style = resolver.computed_style(&body);
    assert_eq!(style.get("max-width"), Some(&ComputedValue::Px(768.0)));
}

#[test]
fn computes_rgba_function_to_hex_with_alpha() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: rgba(255, 0, 0, 0.5); }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    // rgba(255, 0, 0, 0.5) → r=255 g=0 b=0 a=128(0x80)
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("#ff000080".to_string()))
    );
}

#[test]
fn computes_rgba_fully_opaque_to_hex() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: rgba(0, 128, 255, 1); }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("#0080ff".to_string()))
    );
}

#[test]
fn computes_hsl_function_to_hex() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: hsl(0, 100%, 50%); }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    // hsl(0, 100%, 50%) → pure red
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("#ff0000".to_string()))
    );
}

#[test]
fn computes_hsl_green_to_hex() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: hsl(120, 100%, 50%); }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    // hsl(120, 100%, 50%) → pure green
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("#00ff00".to_string()))
    );
}

#[test]
fn computes_hsla_function_to_hex_with_alpha() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: hsla(240, 100%, 50%, 0.5); }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    // hsla(240, 100%, 50%, 0.5) → semi-transparent blue a=128(0x80)
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("#0000ff80".to_string()))
    );
}

#[test]
fn computes_rgb_modern_syntax_with_alpha() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: rgb(255 0 0 / 0.5); }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    // rgb(255 0 0 / 0.5) → semi-transparent red a=128(0x80)
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("#ff000080".to_string()))
    );
}

#[test]
fn computes_rgb_modern_syntax_no_alpha() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: rgb(0 128 255); }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("#0080ff".to_string()))
    );
}

#[test]
fn computes_named_color_coral() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: coral; }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("coral".to_string()))
    );
}

#[test]
fn computes_named_color_crimson() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: crimson; }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("crimson".to_string()))
    );
}

#[test]
fn computes_rgba_percentage_alpha() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: rgba(255, 0, 0, 50%); }").unwrap(),
    );
    let style = resolver.computed_style(&title);
    // 50% alpha = 0.5 → hex alpha 80
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("#ff000080".to_string()))
    );
}

#[test]
fn computes_hsl_wraps_hue_above_360() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: hsl(720, 100%, 50%); }").unwrap(),
    );
    let style = resolver.computed_style(&title);
    // 720 mod 360 = 0 → red
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("#ff0000".to_string()))
    );
}

#[test]
fn computes_hsl_wraps_negative_hue() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: hsl(-120, 100%, 50%); }").unwrap(),
    );
    let style = resolver.computed_style(&title);
    // -120 mod 360 = 240 → blue
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("#0000ff".to_string()))
    );
}

// --- shorthand 展開テスト ---

#[test]
fn expands_margin_1_value() {
    let stylesheet = parse_stylesheet("div { margin: 10px; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    for side in ["margin-top", "margin-right", "margin-bottom", "margin-left"] {
        assert!(
            rule.declarations
                .iter()
                .any(|d| d.name == side && matches!(&d.value, Value::Length(v, u) if *v == 10.0 && u == "px")),
            "{side} not found with 10px"
        );
    }
}

#[test]
fn expands_margin_2_values() {
    let stylesheet = parse_stylesheet("div { margin: 10px 20px; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    // top/bottom = 10px, right/left = 20px
    for side in ["margin-top", "margin-bottom"] {
        assert!(
            rule.declarations
                .iter()
                .any(|d| d.name == side && matches!(&d.value, Value::Length(v, u) if *v == 10.0 && u == "px")),
            "{side} not found with 10px"
        );
    }
    for side in ["margin-right", "margin-left"] {
        assert!(
            rule.declarations
                .iter()
                .any(|d| d.name == side && matches!(&d.value, Value::Length(v, u) if *v == 20.0 && u == "px")),
            "{side} not found with 20px"
        );
    }
}

#[test]
fn expands_margin_3_values() {
    let stylesheet = parse_stylesheet("div { margin: 10px 20px 30px; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    // top=10px, right/left=20px, bottom=30px
    assert!(rule.declarations.iter().any(
        |d| d.name == "margin-top" && matches!(&d.value, Value::Length(v, u) if *v == 10.0 && u == "px")
    ));
    assert!(rule.declarations.iter().any(
        |d| d.name == "margin-right" && matches!(&d.value, Value::Length(v, u) if *v == 20.0 && u == "px")
    ));
    assert!(rule.declarations.iter().any(
        |d| d.name == "margin-bottom" && matches!(&d.value, Value::Length(v, u) if *v == 30.0 && u == "px")
    ));
    assert!(rule.declarations.iter().any(
        |d| d.name == "margin-left" && matches!(&d.value, Value::Length(v, u) if *v == 20.0 && u == "px")
    ));
}

#[test]
fn expands_margin_4_values() {
    let stylesheet = parse_stylesheet("div { margin: 1px 2px 3px 4px; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    let expected = [
        ("margin-top", 1.0f32),
        ("margin-right", 2.0),
        ("margin-bottom", 3.0),
        ("margin-left", 4.0),
    ];
    for (side, px) in &expected {
        assert!(
            rule.declarations
                .iter()
                .any(|d| d.name == *side && matches!(&d.value, Value::Length(v, u) if *v == *px && u == "px")),
            "{side} not found with {px}px"
        );
    }
}

#[test]
fn expands_padding_4_values() {
    let stylesheet = parse_stylesheet("div { padding: 1px 2px 3px 4px; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    let expected = [
        ("padding-top", 1.0f32),
        ("padding-right", 2.0),
        ("padding-bottom", 3.0),
        ("padding-left", 4.0),
    ];
    for (side, px) in &expected {
        assert!(
            rule.declarations
                .iter()
                .any(|d| d.name == *side && matches!(&d.value, Value::Length(v, u) if *v == *px && u == "px")),
            "{side} not found with {px}px"
        );
    }
}

#[test]
fn expands_border_width_4_values() {
    let stylesheet = parse_stylesheet("div { border-width: 1px 2px 3px 4px; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    let expected = [
        ("border-top-width", 1.0f32),
        ("border-right-width", 2.0),
        ("border-bottom-width", 3.0),
        ("border-left-width", 4.0),
    ];
    for (side, px) in &expected {
        assert!(
            rule.declarations
                .iter()
                .any(|d| d.name == *side && matches!(&d.value, Value::Length(v, u) if *v == *px && u == "px")),
            "{side} not found with {px}px"
        );
    }
}

#[test]
fn expands_border_color_2_values() {
    let stylesheet = parse_stylesheet("div { border-color: red blue; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    for side in ["border-top-color", "border-bottom-color"] {
        assert!(
            rule.declarations
                .iter()
                .any(|d| d.name == side && matches!(&d.value, Value::Keyword(v) if v == "red")),
            "{side} not found with red"
        );
    }
    for side in ["border-right-color", "border-left-color"] {
        assert!(
            rule.declarations
                .iter()
                .any(|d| d.name == side && matches!(&d.value, Value::Keyword(v) if v == "blue")),
            "{side} not found with blue"
        );
    }
}

#[test]
fn expands_overflow_1_value() {
    let stylesheet = parse_stylesheet("div { overflow: hidden; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    for prop in ["overflow-x", "overflow-y"] {
        assert!(
            rule.declarations
                .iter()
                .any(|d| d.name == prop && matches!(&d.value, Value::Keyword(v) if v == "hidden")),
            "{prop} not found with hidden"
        );
    }
}

#[test]
fn expands_overflow_2_values() {
    let stylesheet = parse_stylesheet("div { overflow: auto scroll; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "overflow-x" && matches!(&d.value, Value::Keyword(v) if v == "auto")),
        "overflow-x not found with auto"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "overflow-y" && matches!(&d.value, Value::Keyword(v) if v == "scroll")),
        "overflow-y not found with scroll"
    );
}

#[test]
fn expands_flex_shorthand_grow_shrink_basis() {
    let stylesheet = parse_stylesheet("div { flex: 2 1 100px; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-grow" && matches!(&d.value, Value::Number(v) if *v == 2.0)),
        "flex-grow not found with 2"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-shrink" && matches!(&d.value, Value::Number(v) if *v == 1.0)),
        "flex-shrink not found with 1"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-basis" && matches!(&d.value, Value::Length(v, u) if *v == 100.0 && u == "px")),
        "flex-basis not found with 100px"
    );
}

#[test]
fn expands_flex_shorthand_1_value_number() {
    // flex: 2 → flex-grow: 2, flex-shrink: 1, flex-basis: 0
    let stylesheet = parse_stylesheet("div { flex: 2; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-grow" && matches!(&d.value, Value::Number(v) if *v == 2.0)),
        "flex-grow not found with 2"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-shrink" && matches!(&d.value, Value::Number(v) if *v == 1.0)),
        "flex-shrink not found with 1"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-basis" && matches!(&d.value, Value::Number(v) if *v == 0.0)),
        "flex-basis not found with 0"
    );
}

#[test]
fn expands_flex_shorthand_none() {
    // flex: none → flex-grow: 0, flex-shrink: 0, flex-basis: auto
    let stylesheet = parse_stylesheet("div { flex: none; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-grow" && matches!(&d.value, Value::Number(v) if *v == 0.0)),
        "flex-grow not found with 0"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-shrink" && matches!(&d.value, Value::Number(v) if *v == 0.0)),
        "flex-shrink not found with 0"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-basis" && matches!(&d.value, Value::Keyword(v) if v == "auto")),
        "flex-basis not found with auto"
    );
}

#[test]
fn expands_flex_shorthand_auto() {
    // flex: auto → flex-grow: 1, flex-shrink: 1, flex-basis: auto
    let stylesheet = parse_stylesheet("div { flex: auto; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-grow" && matches!(&d.value, Value::Number(v) if *v == 1.0)),
        "flex-grow not found with 1"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-shrink" && matches!(&d.value, Value::Number(v) if *v == 1.0)),
        "flex-shrink not found with 1"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-basis" && matches!(&d.value, Value::Keyword(v) if v == "auto")),
        "flex-basis not found with auto"
    );
}

#[test]
fn expands_flex_shorthand_basis_only() {
    // flex: 100px → flex-grow: 1, flex-shrink: 1, flex-basis: 100px
    let stylesheet = parse_stylesheet("div { flex: 100px; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-grow" && matches!(&d.value, Value::Number(v) if *v == 1.0)),
        "flex-grow not found with 1"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-shrink" && matches!(&d.value, Value::Number(v) if *v == 1.0)),
        "flex-shrink not found with 1"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-basis" && matches!(&d.value, Value::Length(v, u) if *v == 100.0 && u == "px")),
        "flex-basis not found with 100px"
    );
}

#[test]
fn expands_flex_shorthand_grow_basis() {
    // flex: 2 100px → flex-grow: 2, flex-shrink: 1, flex-basis: 100px
    let stylesheet = parse_stylesheet("div { flex: 2 100px; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-grow" && matches!(&d.value, Value::Number(v) if *v == 2.0)),
        "flex-grow not found with 2"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-shrink" && matches!(&d.value, Value::Number(v) if *v == 1.0)),
        "flex-shrink not found with 1"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-basis" && matches!(&d.value, Value::Length(v, u) if *v == 100.0 && u == "px")),
        "flex-basis not found with 100px"
    );
}

// ===== text-decoration shorthand tests =====

#[test]
fn expands_text_decoration_shorthand_underline() {
    let stylesheet = parse_stylesheet("a { text-decoration: underline; }").unwrap();
    let rule = match &stylesheet.rules[0] {
        crate::css::Rule::Style(r) => r,
        _ => panic!("expected style rule"),
    };
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "text-decoration-line"
                && matches!(&d.value, Value::Keyword(v) if v == "underline")),
        "text-decoration-line: underline not found"
    );
}

#[test]
fn expands_text_decoration_shorthand_line_through_with_color() {
    let stylesheet =
        parse_stylesheet("del { text-decoration: line-through red; }").unwrap();
    let rule = match &stylesheet.rules[0] {
        crate::css::Rule::Style(r) => r,
        _ => panic!("expected style rule"),
    };
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "text-decoration-line"
                && matches!(&d.value, Value::Keyword(v) if v == "line-through")),
        "text-decoration-line: line-through not found"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "text-decoration-color"),
        "text-decoration-color not found"
    );
}

#[test]
fn expands_text_decoration_shorthand_solid_style() {
    let stylesheet = parse_stylesheet("u { text-decoration: underline solid; }").unwrap();
    let rule = match &stylesheet.rules[0] {
        crate::css::Rule::Style(r) => r,
        _ => panic!("expected style rule"),
    };
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "text-decoration-style"
                && matches!(&d.value, Value::Keyword(v) if v == "solid")),
        "text-decoration-style: solid not found"
    );
}

#[test]
fn expands_text_decoration_shorthand_none() {
    let stylesheet = parse_stylesheet("a { text-decoration: none; }").unwrap();
    let rule = match &stylesheet.rules[0] {
        crate::css::Rule::Style(r) => r,
        _ => panic!("expected style rule"),
    };
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "text-decoration-line"
                && matches!(&d.value, Value::Keyword(v) if v == "none")),
        "text-decoration-line: none not found"
    );
}

// ===== text-transform compute tests =====

#[test]
fn text_transform_initial_value_is_none() {
    let document = NodeHandle::document();
    let target = NodeHandle::element("p");
    document.append_child(target.clone());
    let mut resolver = StyleResolver::new();
    let style = resolver.computed_style(&target);
    assert_eq!(
        style.get("text-transform"),
        Some(&ComputedValue::Keyword("none".to_string()))
    );
}

#[test]
fn computes_text_transform_uppercase() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { text-transform: uppercase; }").unwrap(),
    );
    let style = resolver.computed_style(&title);
    assert_eq!(
        style.get("text-transform"),
        Some(&ComputedValue::Keyword("uppercase".to_string()))
    );
}

// ===== letter-spacing inheritance tests =====

#[test]
fn letter_spacing_inherits_from_parent() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let span = NodeHandle::element("span");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(span.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("body { letter-spacing: 2px; }").unwrap(),
    );
    let style = resolver.computed_style(&span);
    assert_eq!(
        style.get("letter-spacing"),
        Some(&ComputedValue::Px(2.0)),
        "letter-spacing should inherit from parent"
    );
}

#[test]
fn word_spacing_inherits_from_parent() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let p = NodeHandle::element("p");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(p.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("body { word-spacing: 4px; }").unwrap(),
    );
    let style = resolver.computed_style(&p);
    assert_eq!(
        style.get("word-spacing"),
        Some(&ComputedValue::Px(4.0)),
        "word-spacing should inherit from parent"
    );
}

// --- rem / viewport unit tests ---

#[test]
fn resolves_rem_using_root_font_size() {
    // rem は root element の font-size (デフォルト 16px) を基準にする
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("div { margin-top: 2rem; }").unwrap(),
    );
    // root font-size = 20px → 2rem = 40px
    resolver.set_root_font_size(20.0);
    let style = resolver.computed_style(&div);
    assert_eq!(
        style.get("margin-top"),
        Some(&ComputedValue::Px(40.0)),
        "2rem with root font-size 20px should be 40px"
    );
}

#[test]
fn resolves_rem_default_root_font_size() {
    // root font-size が未設定の場合はデフォルト 16px を使う
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("div { padding-left: 1.5rem; }").unwrap(),
    );
    // デフォルト root font-size 16px → 1.5rem = 24px
    let style = resolver.computed_style(&div);
    assert_eq!(
        style.get("padding-left"),
        Some(&ComputedValue::Px(24.0)),
        "1.5rem with default root font-size 16px should be 24px"
    );
}

#[test]
fn resolves_vw_unit() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("div { width: 50vw; }").unwrap(),
    );
    // viewport 幅 1000px → 50vw = 500px
    resolver.set_viewport(1000.0, 800.0);
    let style = resolver.computed_style(&div);
    assert_eq!(
        style.get("width"),
        Some(&ComputedValue::Px(500.0)),
        "50vw with viewport width 1000px should be 500px"
    );
}

#[test]
fn resolves_vh_unit() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("div { height: 100vh; }").unwrap(),
    );
    // viewport 高さ 600px → 100vh = 600px
    resolver.set_viewport(1200.0, 600.0);
    let style = resolver.computed_style(&div);
    assert_eq!(
        style.get("height"),
        Some(&ComputedValue::Px(600.0)),
        "100vh with viewport height 600px should be 600px"
    );
}

#[test]
fn resolves_dynamic_viewport_height_unit() {
    let document = NodeHandle::document();
    let div = NodeHandle::element("div");
    document.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("div { min-height: 100dvh; }").unwrap(),
    );
    resolver.set_viewport(1280.0, 720.0);

    assert_eq!(
        resolver.computed_style(&div).get("min-height"),
        Some(&ComputedValue::Px(720.0))
    );
}

#[test]
fn matches_tailwind_arbitrary_breakpoint_class() {
    let document = NodeHandle::document();
    let div = NodeHandle::element("div");
    div.set_attribute("class", "min-[851px]:ps-9");
    document.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            r"@media (width >= 851px) { .min-\[851px\]\:ps-9 { padding-inline-start: 36px; } }",
        )
        .unwrap(),
    );
    resolver.set_viewport(1280.0, 720.0);

    assert_eq!(
        resolver.computed_style(&div).get("padding-inline-start"),
        Some(&ComputedValue::Px(36.0))
    );
}

#[test]
fn resolves_tailwind_spacing_variable_in_logical_padding() {
    let document = NodeHandle::document();
    let div = NodeHandle::element("div");
    div.set_attribute("class", "px-4");
    document.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            ":root { --spacing: 0.25rem; } \
             .px-4 { padding-inline: calc(var(--spacing) * 4); }",
        )
        .unwrap(),
    );

    let style = resolver.computed_style(&div);
    assert_eq!(style.get("padding-inline-start"), Some(&ComputedValue::Px(16.0)));
    assert_eq!(style.get("padding-inline-end"), Some(&ComputedValue::Px(16.0)));
}

#[test]
fn logical_padding_start_overrides_earlier_physical_reset() {
    let document = NodeHandle::document();
    let div = NodeHandle::element("div");
    div.set_attribute("class", "desktop-padding");
    document.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "* { padding-left: 0; } \
             .desktop-padding { padding-inline-start: 36px; }",
        )
        .unwrap(),
    );

    let style = resolver.computed_style(&div);
    assert_eq!(style.get("padding-left"), Some(&ComputedValue::Px(36.0)));
    assert_eq!(style.get("padding-inline-start"), Some(&ComputedValue::Px(36.0)));
}

#[test]
fn later_physical_padding_overrides_logical_start() {
    let document = NodeHandle::document();
    let div = NodeHandle::element("div");
    document.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "div { padding-inline-start: 36px; } \
             div { padding-left: 12px; }",
        )
        .unwrap(),
    );

    let style = resolver.computed_style(&div);
    assert_eq!(style.get("padding-left"), Some(&ComputedValue::Px(12.0)));
}

#[test]
fn logical_block_padding_overrides_earlier_physical_reset() {
    let document = NodeHandle::document();
    let div = NodeHandle::element("div");
    document.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "* { padding-top: 0; padding-bottom: 0; } \
             div { padding-block: 40px 12px; }",
        )
        .unwrap(),
    );

    let style = resolver.computed_style(&div);
    assert_eq!(style.get("padding-top"), Some(&ComputedValue::Px(40.0)));
    assert_eq!(style.get("padding-bottom"), Some(&ComputedValue::Px(12.0)));
}

#[test]
fn applies_rules_nested_in_cascade_layer() {
    let document = NodeHandle::document();
    let div = NodeHandle::element("div");
    div.set_attribute("class", "grouped");
    document.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "@layer utilities { .grouped { padding-left: 12px; } }",
        )
        .unwrap(),
    );

    let style = resolver.computed_style(&div);
    assert_eq!(style.get("padding-left"), Some(&ComputedValue::Px(12.0)));
}

#[test]
fn resolves_vmin_unit() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("div { width: 10vmin; }").unwrap(),
    );
    // viewport 1000x600 → vmin = 600px の 1% → 10vmin = 60px
    resolver.set_viewport(1000.0, 600.0);
    let style = resolver.computed_style(&div);
    assert_eq!(
        style.get("width"),
        Some(&ComputedValue::Px(60.0)),
        "10vmin with viewport 1000x600 should be 60px"
    );
}

#[test]
fn resolves_vmax_unit() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("div { width: 10vmax; }").unwrap(),
    );
    // viewport 1000x600 → vmax = 1000px の 1% → 10vmax = 100px
    resolver.set_viewport(1000.0, 600.0);
    let style = resolver.computed_style(&div);
    assert_eq!(
        style.get("width"),
        Some(&ComputedValue::Px(100.0)),
        "10vmax with viewport 1000x600 should be 100px"
    );
}

#[test]
fn resolves_rem_in_font_size() {
    // font-size に rem を使った場合
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("div { font-size: 1.5rem; }").unwrap(),
    );
    // root font-size = 16px → 1.5rem = 24px
    resolver.set_root_font_size(16.0);
    let style = resolver.computed_style(&div);
    assert_eq!(
        style.get("font-size"),
        Some(&ComputedValue::Px(24.0)),
        "1.5rem font-size with root font-size 16px should be 24px"
    );
}

#[test]
fn rem_resolves_from_css_defined_root_font_size() {
    // html の font-size が CSS で指定されていれば、rem はその値を使う
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        // html の font-size を 20px に設定
        parse_stylesheet("html { font-size: 20px; } div { margin-top: 2rem; }").unwrap(),
    );
    // set_root_font_size() を呼ばなくても CSS の html font-size から自動計算される
    let _ = resolver.computed_style(&html); // html のスタイルを先に解決
    let style = resolver.computed_style(&div);
    assert_eq!(
        style.get("margin-top"),
        Some(&ComputedValue::Px(40.0)),
        "2rem should resolve from CSS-defined root font-size of 20px"
    );
}

#[test]
fn rem_on_root_element_uses_own_computed_font_size() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    document.append_child(html.clone());
    html.append_child(body.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("html { font-size: 20px; margin-top: 2rem; }").unwrap(),
    );

    let style = resolver.computed_style(&html);
    assert_eq!(style.get("font-size"), Some(&ComputedValue::Px(20.0)));
    // rem on root should use the root's own computed font-size (20px), not the default 16px
    assert_eq!(
        style.get("margin-top"),
        Some(&ComputedValue::Px(40.0)),
        "2rem on html should be 2 * 20px = 40px"
    );
}

#[test]
fn calc_with_viewport_units() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    document.append_child(html.clone());
    html.append_child(body.clone());

    let mut resolver = StyleResolver::new();
    resolver.set_viewport(1000.0, 800.0);
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("body { width: calc(50vw - 10px); height: calc(10vh + 1rem); }").unwrap(),
    );

    let style = resolver.computed_style(&body);
    // 50vw = 500, - 10px = 490
    assert_eq!(style.get("width"), Some(&ComputedValue::Px(490.0)));
    // 10vh = 80, + 1rem = 16 → 96
    assert_eq!(style.get("height"), Some(&ComputedValue::Px(96.0)));
}

#[test]
fn canonicalizes_grid_track_units_calc_functions_and_named_lines() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    document.append_child(html.clone());
    html.append_child(body.clone());

    let mut resolver = StyleResolver::new();
    resolver.set_viewport(1000.0, 800.0);
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { grid-template-columns: [start] 30vw minmax(2rem, 1fr) repeat(auto-fill, calc(10vw + 20px)) [end]; }",
        )
        .unwrap(),
    );

    let style = resolver.computed_style(&body);
    assert_eq!(
        style.get("grid-template-columns"),
        Some(&ComputedValue::Keyword(
            "[start] 300px minmax(32px, 1fr) repeat(auto-fill, 120px) [end]".to_string()
        ))
    );
}

#[test]
fn canonicalizes_grid_calc_multiplication_and_mixed_percentages() {
    let (_document, body, _title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { grid-template-rows: calc(20px * 3) calc(25% + 10px); }",
        )
        .unwrap(),
    );

    let style = resolver.computed_style(&body);
    assert_eq!(
        style.get("grid-template-rows"),
        Some(&ComputedValue::Keyword(
            "60px calc(10px + 25%)".to_string()
        ))
    );
}

#[test]
fn canonicalizes_clip_path_inset_and_webkit_alias() {
    let (_document, body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { clip-path: inset(calc(1rem + 2px) 10% calc(25% - 5px) 3px round 8px); } \
             h1 { -webkit-clip-path: inset(0 0 100% 0); }",
        )
        .unwrap(),
    );

    let body_style = resolver.computed_style(&body);
    assert_eq!(
        body_style.get("clip-path"),
        Some(&ComputedValue::Keyword(
            "inset(18px 10% calc(-5px + 25%) 3px round 8px)".to_string()
        ))
    );

    let title_style = resolver.computed_style(&title);
    assert_eq!(
        title_style.get("clip-path"),
        Some(&ComputedValue::Keyword("inset(0 0 100% 0)".to_string()))
    );
    assert_eq!(title_style.get("-webkit-clip-path"), None);
    assert!(is_supported_property("clip-path"));
    assert!(is_supported_property("-webkit-clip-path"));
}

#[test]
fn canonicalizes_clip_shape_and_mask_layer_values() {
    let (_document, body, _title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { clip-path: circle(50% at 50% 50%); \
             mask-image: linear-gradient(to right, black, transparent), url(mask.svg); \
             mask-mode: luminance, alpha; mask-composite: add; }",
        )
        .unwrap(),
    );
    let style = resolver.computed_style(&body);
    assert_eq!(
        style.get("clip-path"),
        Some(&ComputedValue::Keyword(
            "circle(50% at 50% 50%)".to_string()
        ))
    );
    assert_eq!(
        style.get("mask-image"),
        Some(&ComputedValue::Keyword(
            "linear-gradient(to right, black, transparent), url(mask.svg)".to_string()
        ))
    );
    assert_eq!(
        style.get("mask-mode"),
        Some(&ComputedValue::Keyword("luminance, alpha".to_string()))
    );
    assert_eq!(
        style.get("mask-composite"),
        Some(&ComputedValue::Keyword("add".to_string()))
    );
    assert!(is_supported_property("mask-mode"));
    assert!(is_supported_property("mask-composite"));
}

#[test]
fn preserves_mask_position_size_and_repeat_layers() {
    let (_document, body, _title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { mask-image: url(a.svg), url(b.svg); \
             mask-position: left, right; mask-size: contain, cover; \
             mask-repeat: no-repeat, repeat; }",
        )
        .unwrap(),
    );
    let style = resolver.computed_style(&body);
    assert_eq!(
        style.get("mask-position-x"),
        Some(&ComputedValue::Keyword("left, right".to_string()))
    );
    assert_eq!(
        style.get("mask-position-y"),
        Some(&ComputedValue::Keyword("center, center".to_string()))
    );
    assert_eq!(
        style.get("mask-size"),
        Some(&ComputedValue::Keyword("contain, cover".to_string()))
    );
    assert_eq!(
        style.get("mask-repeat"),
        Some(&ComputedValue::Keyword("no-repeat, repeat".to_string()))
    );
}

#[test]
fn canonicalizes_uppercase_clip_shape_function_names() {
    let (_document, body, _title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("body { clip-path: CIRCLE(50% AT 50% 50%); }").unwrap(),
    );
    let style = resolver.computed_style(&body);
    assert_eq!(
        style.get("clip-path"),
        Some(&ComputedValue::Keyword("circle(50% AT 50% 50%)".to_string()))
    );
}

#[test]
fn invalid_clip_shape_and_mask_mode_declarations_are_ignored() {
    let (_document, body, _title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { clip-path: made-up(1px); mask-mode: invalid; \
             clip-path: polygon(0% 0%, 100% 0%, 0% 100%); }",
        )
        .unwrap(),
    );
    let style = resolver.computed_style(&body);
    assert_eq!(
        style.get("clip-path"),
        Some(&ComputedValue::Keyword(
            "polygon(0% 0%, 100% 0%, 0% 100%)".to_string()
        ))
    );
    assert_eq!(
        style.get("mask-mode"),
        Some(&ComputedValue::Keyword("match-source".to_string()))
    );
}

#[test]
fn validates_clip_shape_function_arguments() {
    assert!(!supports_declaration("clip-path", "circle(not-a-length)"));
    assert!(!supports_declaration("clip-path", "inset(1px round)"));
    assert!(!supports_declaration(
        "clip-path",
        "polygon(0% 0%, 100% 0%)"
    ));
    assert!(!supports_declaration(
        "clip-path",
        "polygon(0% 0%, 100% 0%, 0% 100% round 2px)"
    ));
    assert!(supports_declaration("clip-path", "inset(1px)"));
    assert!(supports_declaration(
        "clip-path",
        "circle(closest-side at 25% 50%)"
    ));
    assert!(supports_declaration(
        "clip-path",
        "circle(10px at 1rem 2rem)"
    ));
    assert!(supports_declaration(
        "clip-path",
        "polygon(0% 0%, 100% 0%, 0% 100%)"
    ));
}

// --- border-radius shorthand 展開テスト ---

#[test]
fn expands_border_radius_1_value() {
    let stylesheet = parse_stylesheet("div { border-radius: 8px; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    for corner in [
        "border-top-left-radius",
        "border-top-right-radius",
        "border-bottom-right-radius",
        "border-bottom-left-radius",
    ] {
        assert!(
            rule.declarations
                .iter()
                .any(|d| d.name == corner && matches!(&d.value, Value::Length(v, u) if *v == 8.0 && u == "px")),
            "{corner} not found with 8px"
        );
    }
}

#[test]
fn expands_border_radius_2_values() {
    // 2値: TL/BR = 10px, TR/BL = 20px
    let stylesheet = parse_stylesheet("div { border-radius: 10px 20px; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    for corner in ["border-top-left-radius", "border-bottom-right-radius"] {
        assert!(
            rule.declarations
                .iter()
                .any(|d| d.name == corner && matches!(&d.value, Value::Length(v, u) if *v == 10.0 && u == "px")),
            "{corner} not found with 10px"
        );
    }
    for corner in ["border-top-right-radius", "border-bottom-left-radius"] {
        assert!(
            rule.declarations
                .iter()
                .any(|d| d.name == corner && matches!(&d.value, Value::Length(v, u) if *v == 20.0 && u == "px")),
            "{corner} not found with 20px"
        );
    }
}

#[test]
fn expands_border_radius_3_values() {
    // 3値: TL=10px, TR/BL=20px, BR=30px
    let stylesheet = parse_stylesheet("div { border-radius: 10px 20px 30px; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    assert!(rule.declarations.iter().any(
        |d| d.name == "border-top-left-radius" && matches!(&d.value, Value::Length(v, u) if *v == 10.0 && u == "px")
    ));
    assert!(rule.declarations.iter().any(
        |d| d.name == "border-top-right-radius" && matches!(&d.value, Value::Length(v, u) if *v == 20.0 && u == "px")
    ));
    assert!(rule.declarations.iter().any(
        |d| d.name == "border-bottom-right-radius" && matches!(&d.value, Value::Length(v, u) if *v == 30.0 && u == "px")
    ));
    assert!(rule.declarations.iter().any(
        |d| d.name == "border-bottom-left-radius" && matches!(&d.value, Value::Length(v, u) if *v == 20.0 && u == "px")
    ));
}

#[test]
fn expands_border_radius_4_values() {
    // 4値: TL/TR/BR/BL = 1/2/3/4px
    let stylesheet = parse_stylesheet("div { border-radius: 1px 2px 3px 4px; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    let expected = [
        ("border-top-left-radius", 1.0f32),
        ("border-top-right-radius", 2.0),
        ("border-bottom-right-radius", 3.0),
        ("border-bottom-left-radius", 4.0),
    ];
    for (corner, px) in &expected {
        assert!(
            rule.declarations
                .iter()
                .any(|d| d.name == *corner && matches!(&d.value, Value::Length(v, u) if *v == *px && u == "px")),
            "{corner} not found with {px}px"
        );
    }
}

#[test]
fn border_radius_longhand_supported_properties() {
    assert!(is_supported_property("border-top-left-radius"));
    assert!(is_supported_property("border-top-right-radius"));
    assert!(is_supported_property("border-bottom-right-radius"));
    assert!(is_supported_property("border-bottom-left-radius"));
}

// ---- list-style tests ----

#[test]
fn list_style_type_and_position_are_supported_properties() {
    assert!(is_supported_property("list-style-type"));
    assert!(is_supported_property("list-style-position"));
    assert!(is_supported_property("list-style-image"));
}

#[test]
fn ua_defaults_set_disc_for_ul() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let ul = NodeHandle::element("ul");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(ul.clone());

    let mut resolver = StyleResolver::new();
    let style = resolver.computed_style(&ul);

    assert_eq!(
        style.get("list-style-type"),
        Some(&ComputedValue::Keyword("disc".to_string()))
    );
    assert_eq!(
        style.get("list-style-position"),
        Some(&ComputedValue::Keyword("outside".to_string()))
    );
}

#[test]
fn ua_defaults_set_decimal_for_ol() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let ol = NodeHandle::element("ol");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(ol.clone());

    let mut resolver = StyleResolver::new();
    let style = resolver.computed_style(&ol);

    assert_eq!(
        style.get("list-style-type"),
        Some(&ComputedValue::Keyword("decimal".to_string()))
    );
}

#[test]
fn ua_defaults_set_display_list_item_for_li() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let ul = NodeHandle::element("ul");
    let li = NodeHandle::element("li");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(ul.clone());
    ul.append_child(li.clone());

    let mut resolver = StyleResolver::new();
    let style = resolver.computed_style(&li);

    assert_eq!(
        style.get("display"),
        Some(&ComputedValue::Keyword("list-item".to_string()))
    );
}

#[test]
fn list_style_type_inherits_from_parent() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let ul = NodeHandle::element("ul");
    let li = NodeHandle::element("li");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(ul.clone());
    ul.append_child(li.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("ul { list-style-type: square; }").unwrap(),
    );
    let li_style = resolver.computed_style(&li);

    assert_eq!(
        li_style.get("list-style-type"),
        Some(&ComputedValue::Keyword("square".to_string()))
    );
}

#[test]
fn list_style_type_none_overrides_ua_default() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let ul = NodeHandle::element("ul");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(ul.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("ul { list-style-type: none; }").unwrap(),
    );
    let style = resolver.computed_style(&ul);

    assert_eq!(
        style.get("list-style-type"),
        Some(&ComputedValue::Keyword("none".to_string()))
    );
}

// ── @media query integration tests ───────────────────────────────────────────

fn make_div_tree() -> (NodeHandle, NodeHandle) {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(div.clone());
    (document, div)
}

#[test]
fn media_screen_type_applies() {
    let (_document, div) = make_div_tree();
    let mut resolver = StyleResolver::new();
    resolver.set_viewport(1024.0, 768.0);
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("@media screen { div { color: red; } }").unwrap(),
    );
    let style = resolver.computed_style(&div);
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("red".to_string())),
        "@media screen should apply on a screen viewport"
    );
}

#[test]
fn media_print_type_does_not_apply() {
    let (_document, div) = make_div_tree();
    let mut resolver = StyleResolver::new();
    resolver.set_viewport(1024.0, 768.0);
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("@media print { div { color: red; } }").unwrap(),
    );
    let style = resolver.computed_style(&div);
    // color should not be "red" (print rule must not apply on screen)
    assert_ne!(
        style.get("color"),
        Some(&ComputedValue::Color("red".to_string())),
        "@media print should not apply on a screen viewport"
    );
}

#[test]
fn media_max_width_applies_when_viewport_fits() {
    let (_document, div) = make_div_tree();
    let mut resolver = StyleResolver::new();
    // viewport width (600) ≤ max-width (768) → should apply.
    resolver.set_viewport(600.0, 900.0);
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("@media (max-width: 768px) { div { color: blue; } }").unwrap(),
    );
    let style = resolver.computed_style(&div);
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("blue".to_string())),
        "rule should apply when viewport width ≤ max-width"
    );
}

#[test]
fn media_max_width_does_not_apply_when_viewport_too_wide() {
    let (_document, div) = make_div_tree();
    let mut resolver = StyleResolver::new();
    // viewport width (1024) > max-width (768) → should NOT apply.
    resolver.set_viewport(1024.0, 768.0);
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("@media (max-width: 768px) { div { color: blue; } }").unwrap(),
    );
    let style = resolver.computed_style(&div);
    assert_ne!(
        style.get("color"),
        Some(&ComputedValue::Color("blue".to_string())),
        "rule should not apply when viewport width > max-width"
    );
}

#[test]
fn media_min_width_applies_when_viewport_wide_enough() {
    let (_document, div) = make_div_tree();
    let mut resolver = StyleResolver::new();
    resolver.set_viewport(1280.0, 800.0);
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("@media (min-width: 1024px) { div { color: green; } }").unwrap(),
    );
    let style = resolver.computed_style(&div);
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("green".to_string())),
        "rule should apply when viewport width ≥ min-width"
    );
}

#[test]
fn media_min_width_does_not_apply_when_viewport_too_narrow() {
    let (_document, div) = make_div_tree();
    let mut resolver = StyleResolver::new();
    resolver.set_viewport(800.0, 600.0);
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("@media (min-width: 1024px) { div { color: green; } }").unwrap(),
    );
    let style = resolver.computed_style(&div);
    assert_ne!(
        style.get("color"),
        Some(&ComputedValue::Color("green".to_string())),
        "rule should not apply when viewport width < min-width"
    );
}

#[test]
fn media_orientation_portrait_applies() {
    let (_document, div) = make_div_tree();
    let mut resolver = StyleResolver::new();
    // height (900) > width (600) → portrait.
    resolver.set_viewport(600.0, 900.0);
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("@media (orientation: portrait) { div { color: purple; } }").unwrap(),
    );
    let style = resolver.computed_style(&div);
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("purple".to_string())),
        "@media (orientation: portrait) should apply in portrait viewport"
    );
}

#[test]
fn media_orientation_portrait_does_not_apply_in_landscape() {
    let (_document, div) = make_div_tree();
    let mut resolver = StyleResolver::new();
    // width (1024) > height (768) → landscape.
    resolver.set_viewport(1024.0, 768.0);
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("@media (orientation: portrait) { div { color: purple; } }").unwrap(),
    );
    let style = resolver.computed_style(&div);
    assert_ne!(
        style.get("color"),
        Some(&ComputedValue::Color("purple".to_string())),
        "@media (orientation: portrait) should not apply in landscape viewport"
    );
}

#[test]
fn media_not_print_applies_on_screen() {
    let (_document, div) = make_div_tree();
    let mut resolver = StyleResolver::new();
    resolver.set_viewport(1024.0, 768.0);
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("@media not print { div { color: teal; } }").unwrap(),
    );
    let style = resolver.computed_style(&div);
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("teal".to_string())),
        "@media not print should apply on a screen viewport"
    );
}

#[test]
fn media_and_conditions_all_must_match() {
    let (_document, div) = make_div_tree();
    let mut resolver = StyleResolver::new();
    // Both min-width (600) and max-width (1200) satisfied at 1024.
    resolver.set_viewport(1024.0, 768.0);
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "@media (min-width: 600px) and (max-width: 1200px) { div { color: orange; } }",
        )
        .unwrap(),
    );
    let style = resolver.computed_style(&div);
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("orange".to_string())),
        "all conditions must be met for the rule to apply"
    );
}

#[test]
fn media_and_conditions_one_mismatch_blocks_rule() {
    let (_document, div) = make_div_tree();
    let mut resolver = StyleResolver::new();
    // min-width (600) satisfied but max-width (1200) NOT satisfied at 1400.
    resolver.set_viewport(1400.0, 768.0);
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "@media (min-width: 600px) and (max-width: 1200px) { div { color: orange; } }",
        )
        .unwrap(),
    );
    let style = resolver.computed_style(&div);
    assert_ne!(
        style.get("color"),
        Some(&ComputedValue::Color("orange".to_string())),
        "rule should not apply when one condition is not met"
    );
}

#[test]
fn media_comma_list_applies_when_any_query_matches() {
    let (_document, div) = make_div_tree();
    let mut resolver = StyleResolver::new();
    // "print" doesn't match, but "screen" does.
    resolver.set_viewport(1024.0, 768.0);
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("@media print, screen { div { color: navy; } }").unwrap(),
    );
    let style = resolver.computed_style(&div);
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("navy".to_string())),
        "comma-separated media query should apply when any query matches"
    );
}

#[test]
fn media_only_screen_applies() {
    // `only screen and (max-width: 768px)` — `only` is a CSS2 modifier and must
    // be stripped; the rule should behave exactly like `screen and (max-width: 768px)`.
    let (_document, div) = make_div_tree();
    let mut resolver = StyleResolver::new();
    resolver.set_viewport(600.0, 900.0);
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "@media only screen and (max-width: 768px) { div { color: magenta; } }",
        )
        .unwrap(),
    );
    let style = resolver.computed_style(&div);
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("magenta".to_string())),
        "@media only screen should apply on a screen viewport narrower than max-width"
    );
}

#[test]
fn media_prefers_color_scheme_dark_applies_in_dark_mode() {
    let (_document, div) = make_div_tree();
    let mut resolver = StyleResolver::new();
    resolver.set_viewport(1024.0, 768.0);
    resolver.set_color_scheme_dark(true);
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "@media (prefers-color-scheme: dark) { div { color: white; } }",
        )
        .unwrap(),
    );
    let style = resolver.computed_style(&div);
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("white".to_string())),
        "@media (prefers-color-scheme: dark) should apply when dark mode is set"
    );
}

#[test]
fn media_prefers_color_scheme_dark_does_not_apply_in_light_mode() {
    let (_document, div) = make_div_tree();
    let mut resolver = StyleResolver::new();
    resolver.set_viewport(1024.0, 768.0);
    // Default is light mode (color_scheme_dark = false).
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "@media (prefers-color-scheme: dark) { div { color: white; } }",
        )
        .unwrap(),
    );
    let style = resolver.computed_style(&div);
    assert_ne!(
        style.get("color"),
        Some(&ComputedValue::Color("white".to_string())),
        "@media (prefers-color-scheme: dark) must not apply in light mode"
    );
}

#[test]
fn media_without_viewport_max_width_zero_matches() {
    // When no viewport is set (0×0), a (max-width: 0px) query should match
    // because viewport_width (0) ≤ 0.
    let (_document, div) = make_div_tree();
    let mut resolver = StyleResolver::new();
    // No set_viewport call → defaults to 0×0.
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("@media (max-width: 0px) { div { color: pink; } }").unwrap(),
    );
    let style = resolver.computed_style(&div);
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("pink".to_string())),
        "max-width: 0px should match a 0-width viewport"
    );
}

// ── media query parse cache tests ────────────────────────────────────────────

/// Builds a DOM tree with multiple sibling div elements under a common parent.
fn make_multi_div_tree(count: usize) -> (NodeHandle, Vec<NodeHandle>) {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    document.append_child(html.clone());
    html.append_child(body.clone());
    let mut divs = Vec::with_capacity(count);
    for _ in 0..count {
        let div = NodeHandle::element("div");
        body.append_child(div.clone());
        divs.push(div);
    }
    (document, divs)
}

#[test]
fn media_query_cache_populated_after_resolution() {
    // After resolving styles for any node, the media query cache must contain
    // an entry for each distinct @media prelude string encountered.
    let (_document, divs) = make_multi_div_tree(3);
    let mut resolver = StyleResolver::new();
    resolver.set_viewport(1024.0, 768.0);
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("@media screen { div { color: red; } }").unwrap(),
    );
    // Before resolving, cache is empty.
    assert_eq!(resolver.media_query_cache_len(), 0, "cache should be empty before first resolution");

    // Resolve the first div — cache should be populated.
    let _ = resolver.computed_style(&divs[0]);
    assert_eq!(
        resolver.media_query_cache_len(),
        1,
        "cache should contain one entry after resolving the first div"
    );

    // Resolve remaining divs — cache must not grow (same prelude string).
    let _ = resolver.computed_style(&divs[1]);
    let _ = resolver.computed_style(&divs[2]);
    assert_eq!(
        resolver.media_query_cache_len(),
        1,
        "cache size must remain 1 when the same @media prelude is reused"
    );
}

#[test]
fn media_query_cache_consistent_results_across_nodes() {
    // Results computed with and without the cache must be identical.
    // We verify by resolving the same stylesheet for many sibling nodes and
    // checking that all nodes receive the expected computed value.
    let (_document, divs) = make_multi_div_tree(5);
    let mut resolver = StyleResolver::new();
    resolver.set_viewport(800.0, 600.0);
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("@media (max-width: 1024px) { div { color: blue; } }").unwrap(),
    );

    for (i, div) in divs.iter().enumerate() {
        let style = resolver.computed_style(div);
        assert_eq!(
            style.get("color"),
            Some(&ComputedValue::Color("blue".to_string())),
            "div[{}] should have color:blue (viewport 800px ≤ max-width 1024px)", i
        );
    }
}

#[test]
fn media_query_cache_multiple_preludes() {
    // Two distinct @media blocks must each get their own cache entry.
    let (_document, divs) = make_multi_div_tree(2);
    let mut resolver = StyleResolver::new();
    resolver.set_viewport(1024.0, 768.0);
    // Two different prelude strings.
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "@media screen { div { color: red; } } @media print { div { color: blue; } }",
        )
        .unwrap(),
    );

    let _ = resolver.computed_style(&divs[0]);
    assert_eq!(
        resolver.media_query_cache_len(),
        2,
        "two distinct @media preludes should produce two cache entries"
    );
}

// ===== flex-flow shorthand tests =====

#[test]
fn expands_flex_flow_shorthand_direction_and_wrap() {
    // flex-flow: row wrap → flex-direction: row, flex-wrap: wrap
    let stylesheet = parse_stylesheet("div { flex-flow: row wrap; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-direction"
                && matches!(&d.value, Value::Keyword(v) if v == "row")),
        "flex-direction: row not found"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-wrap"
                && matches!(&d.value, Value::Keyword(v) if v == "wrap")),
        "flex-wrap: wrap not found"
    );
}

#[test]
fn expands_flex_flow_shorthand_direction_only() {
    // flex-flow: column → flex-direction: column, flex-wrap: nowrap (initial)
    let stylesheet = parse_stylesheet("div { flex-flow: column; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-direction"
                && matches!(&d.value, Value::Keyword(v) if v == "column")),
        "flex-direction: column not found"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-wrap"
                && matches!(&d.value, Value::Keyword(v) if v == "nowrap")),
        "flex-wrap: nowrap (initial) not found"
    );
}

#[test]
fn expands_flex_flow_shorthand_wrap_only() {
    // flex-flow: wrap → flex-direction: row (initial), flex-wrap: wrap
    let stylesheet = parse_stylesheet("div { flex-flow: wrap; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-direction"
                && matches!(&d.value, Value::Keyword(v) if v == "row")),
        "flex-direction: row (initial) not found"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-wrap"
                && matches!(&d.value, Value::Keyword(v) if v == "wrap")),
        "flex-wrap: wrap not found"
    );
}

#[test]
fn expands_flex_flow_shorthand_column_reverse_wrap_reverse() {
    // flex-flow: column-reverse wrap-reverse
    let stylesheet = parse_stylesheet("div { flex-flow: column-reverse wrap-reverse; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-direction"
                && matches!(&d.value, Value::Keyword(v) if v == "column-reverse")),
        "flex-direction: column-reverse not found"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-wrap"
                && matches!(&d.value, Value::Keyword(v) if v == "wrap-reverse")),
        "flex-wrap: wrap-reverse not found"
    );
}

#[test]
fn expands_flex_flow_shorthand_initial_keyword() {
    // flex-flow: initial → both longhands receive initial
    let stylesheet = parse_stylesheet("div { flex-flow: initial; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-direction"
                && matches!(&d.value, Value::Keyword(v) if v == "initial")),
        "flex-direction: initial not found"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-wrap"
                && matches!(&d.value, Value::Keyword(v) if v == "initial")),
        "flex-wrap: initial not found"
    );
}

#[test]
fn inherits_font_weight_from_parent() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let child = NodeHandle::element("span");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(child.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("body { font-weight: bold; }").unwrap(),
    );
    let style = resolver.computed_style(&child);
    assert_eq!(
        style.get("font-weight"),
        Some(&ComputedValue::Keyword("bold".to_string())),
        "font-weight should inherit from parent"
    );
}

#[test]
fn inherits_font_style_from_parent() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let child = NodeHandle::element("span");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(child.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("body { font-style: italic; }").unwrap(),
    );
    let style = resolver.computed_style(&child);
    assert_eq!(
        style.get("font-style"),
        Some(&ComputedValue::Keyword("italic".to_string())),
        "font-style should inherit from parent"
    );
}

#[test]
fn inherits_text_align_from_parent() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let child = NodeHandle::element("div");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(child.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("body { text-align: center; }").unwrap(),
    );
    let style = resolver.computed_style(&child);
    assert_eq!(
        style.get("text-align"),
        Some(&ComputedValue::Keyword("center".to_string())),
        "text-align should inherit from parent"
    );
}

#[test]
fn inherits_visibility_from_parent() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let child = NodeHandle::element("span");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(child.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("body { visibility: hidden; }").unwrap(),
    );
    let style = resolver.computed_style(&child);
    assert_eq!(
        style.get("visibility"),
        Some(&ComputedValue::Keyword("hidden".to_string())),
        "visibility should inherit from parent"
    );
}

#[test]
fn inherits_border_collapse_from_parent() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let table = NodeHandle::element("table");
    let tr = NodeHandle::element("tr");
    let td = NodeHandle::element("td");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(table.clone());
    table.append_child(tr.clone());
    tr.append_child(td.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("table { border-collapse: collapse; }").unwrap(),
    );
    let style = resolver.computed_style(&td);
    assert_eq!(
        style.get("border-collapse"),
        Some(&ComputedValue::Keyword("collapse".to_string())),
        "border-collapse should inherit from table to td"
    );
}

#[test]
fn child_override_prevents_inheritance() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let child = NodeHandle::element("span");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(child.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("body { font-weight: bold; } span { font-weight: normal; }").unwrap(),
    );
    let style = resolver.computed_style(&child);
    assert_eq!(
        style.get("font-weight"),
        Some(&ComputedValue::Keyword("normal".to_string())),
        "explicit child font-weight should override inherited value"
    );
}

#[test]
fn inherits_direction_from_parent() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let child = NodeHandle::element("div");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(child.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("body { direction: rtl; }").unwrap(),
    );
    let style = resolver.computed_style(&child);
    assert_eq!(
        style.get("direction"),
        Some(&ComputedValue::Keyword("rtl".to_string())),
        "direction should inherit from parent"
    );
}

#[test]
fn inherits_text_indent_from_parent() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let child = NodeHandle::element("p");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(child.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("body { text-indent: 24px; }").unwrap(),
    );
    let style = resolver.computed_style(&child);
    assert_eq!(
        style.get("text-indent"),
        Some(&ComputedValue::Px(24.0)),
        "text-indent should inherit from parent"
    );
}

#[test]
fn inherits_text_decoration_line_from_parent() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let child = NodeHandle::element("span");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(child.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("body { text-decoration-line: underline; }").unwrap(),
    );
    let style = resolver.computed_style(&child);
    assert_eq!(
        style.get("text-decoration-line"),
        Some(&ComputedValue::Keyword("underline".to_string())),
        "text-decoration-line should inherit from parent"
    );
}

#[test]
fn author_css_overrides_heading_ua_font_weight() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let h1 = NodeHandle::element("h1");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(h1.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { font-weight: normal; }").unwrap(),
    );
    let style = resolver.computed_style(&h1);
    assert_eq!(
        style.get("font-weight"),
        Some(&ComputedValue::Keyword("normal".to_string())),
        "author CSS font-weight:normal should override UA bold for h1"
    );
}

#[test]
fn author_css_overrides_heading_ua_font_size() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let h1 = NodeHandle::element("h1");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(h1.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { font-size: 12px; }").unwrap(),
    );
    let style = resolver.computed_style(&h1);
    assert_eq!(
        style.get("font-size"),
        Some(&ComputedValue::Px(12.0)),
        "author CSS font-size:12px should override UA 2em for h1"
    );
}

#[test]
fn author_css_overrides_heading_ua_margin() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let h1 = NodeHandle::element("h1");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(h1.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { margin: 0; }").unwrap(),
    );
    let style = resolver.computed_style(&h1);
    let margin_top = style.get("margin-top");
    // margin: 0 may compute as Px(0.0) or Number(0.0) depending on unitless zero handling.
    let is_zero = matches!(
        margin_top,
        Some(ComputedValue::Px(v)) | Some(ComputedValue::Number(v)) if *v == 0.0
    );
    assert!(is_zero, "author CSS margin:0 should override UA margin for h1, got {:?}", margin_top);
}

#[test]
fn ua_defaults_blockquote_has_margin() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let bq = NodeHandle::element("blockquote");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(bq.clone());

    let mut resolver = StyleResolver::new();
    let style = resolver.computed_style(&bq);
    assert_eq!(style.get("margin-left"), Some(&ComputedValue::Px(40.0)));
}

#[test]
fn ua_defaults_pre_has_monospace_and_whitespace_pre() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let pre = NodeHandle::element("pre");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(pre.clone());

    let mut resolver = StyleResolver::new();
    let style = resolver.computed_style(&pre);
    assert_eq!(style.get("font-family"), Some(&ComputedValue::Keyword("monospace".to_string())));
    assert_eq!(style.get("white-space"), Some(&ComputedValue::Keyword("pre".to_string())));
}

#[test]
fn ua_defaults_th_is_bold_centered() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let table = NodeHandle::element("table");
    let tr = NodeHandle::element("tr");
    let th = NodeHandle::element("th");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(table.clone());
    table.append_child(tr.clone());
    tr.append_child(th.clone());

    let mut resolver = StyleResolver::new();
    let style = resolver.computed_style(&th);
    assert_eq!(style.get("font-weight"), Some(&ComputedValue::Keyword("bold".to_string())));
    assert_eq!(style.get("text-align"), Some(&ComputedValue::Keyword("center".to_string())));
}

#[test]
fn ua_defaults_a_has_underline_and_blue() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let a = NodeHandle::element("a");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(a.clone());

    let mut resolver = StyleResolver::new();
    let style = resolver.computed_style(&a);
    assert_eq!(style.get("text-decoration-line"), Some(&ComputedValue::Keyword("underline".to_string())));
    assert_eq!(style.get("color"), Some(&ComputedValue::Color("#0000ee".to_string())));
}

#[test]
fn ua_defaults_button_is_inline_block_bordered() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let button = NodeHandle::element("button");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(button.clone());

    let mut resolver = StyleResolver::new();
    let style = resolver.computed_style(&button);
    assert_eq!(
        style.get("display"),
        Some(&ComputedValue::Keyword("inline-block".to_string()))
    );
    assert_eq!(
        style.get("background-color"),
        Some(&ComputedValue::Color("#efefef".to_string()))
    );
    assert_eq!(
        style.get("text-align"),
        Some(&ComputedValue::Keyword("center".to_string()))
    );
    for side in ["top", "right", "bottom", "left"] {
        assert_eq!(
            style.get(&format!("border-{side}-width")),
            Some(&ComputedValue::Px(2.0)),
            "button border-{side}-width"
        );
        assert_eq!(
            style.get(&format!("border-{side}-style")),
            Some(&ComputedValue::Keyword("solid".to_string()))
        );
        assert_eq!(
            style.get(&format!("border-{side}-color")),
            Some(&ComputedValue::Color("#767676".to_string()))
        );
    }
    assert_eq!(style.get("padding-top"), Some(&ComputedValue::Px(1.0)));
    assert_eq!(style.get("padding-right"), Some(&ComputedValue::Px(6.0)));
    assert_eq!(style.get("padding-bottom"), Some(&ComputedValue::Px(1.0)));
    assert_eq!(style.get("padding-left"), Some(&ComputedValue::Px(6.0)));
}

#[test]
fn ua_defaults_html5_media_element_visibility() {
    let video = NodeHandle::element("video");
    let canvas = NodeHandle::element("canvas");
    let audio = NodeHandle::element("audio");
    let controlled_audio = NodeHandle::element("audio");
    controlled_audio.set_attribute("controls", "");
    let source = NodeHandle::element("source");
    let picture = NodeHandle::element("picture");
    let mut resolver = StyleResolver::new();

    for element in [&video, &canvas, &controlled_audio, &picture] {
        assert_eq!(
            resolver.computed_style(element).get("display"),
            Some(&ComputedValue::Keyword("inline-block".to_string()))
        );
    }
    for element in [&audio, &source] {
        assert_eq!(
            resolver.computed_style(element).get("display"),
            Some(&ComputedValue::Keyword("none".to_string()))
        );
    }
}

#[test]
fn ua_defaults_html5_interactive_element_visibility() {
    let details = NodeHandle::element("details");
    let summary = NodeHandle::element("summary");
    let content = NodeHandle::element("div");
    details.append_child(summary.clone());
    details.append_child(content.clone());
    let closed_dialog = NodeHandle::element("dialog");
    let open_dialog = NodeHandle::element("dialog");
    open_dialog.set_attribute("open", "");
    let time = NodeHandle::element("time");
    let progress = NodeHandle::element("progress");
    let meter = NodeHandle::element("meter");

    let mut resolver = StyleResolver::new();
    assert_eq!(
        resolver.computed_style(&details).get("display"),
        Some(&ComputedValue::Keyword("block".to_string()))
    );
    assert_eq!(
        resolver.computed_style(&summary).get("display"),
        Some(&ComputedValue::Keyword("list-item".to_string()))
    );
    assert_eq!(
        resolver.computed_style(&content).get("display"),
        Some(&ComputedValue::Keyword("none".to_string()))
    );
    assert_eq!(
        resolver.computed_style(&closed_dialog).get("display"),
        Some(&ComputedValue::Keyword("none".to_string()))
    );
    assert_eq!(
        resolver.computed_style(&open_dialog).get("display"),
        Some(&ComputedValue::Keyword("block".to_string()))
    );
    assert_eq!(
        resolver.computed_style(&time).get("display"),
        Some(&ComputedValue::Keyword("inline".to_string()))
    );
    for indicator in [&progress, &meter] {
        let style = resolver.computed_style(indicator);
        assert_eq!(
            style.get("display"),
            Some(&ComputedValue::Keyword("inline-block".to_string()))
        );
        assert_eq!(style.get("width"), Some(&ComputedValue::Px(160.0)));
        assert_eq!(style.get("height"), Some(&ComputedValue::Px(16.0)));
    }

    details.set_attribute("open", "");
    let mut open_resolver = StyleResolver::new();
    assert_ne!(
        open_resolver.computed_style(&content).get("display"),
        Some(&ComputedValue::Keyword("none".to_string()))
    );
}

#[test]
fn ua_defaults_textarea_is_inline_block_bordered() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let textarea = NodeHandle::element("textarea");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(textarea.clone());

    let mut resolver = StyleResolver::new();
    let style = resolver.computed_style(&textarea);
    assert_eq!(
        style.get("display"),
        Some(&ComputedValue::Keyword("inline-block".to_string()))
    );
    assert_eq!(
        style.get("background-color"),
        Some(&ComputedValue::Color("white".to_string()))
    );
    for side in ["top", "right", "bottom", "left"] {
        assert_eq!(
            style.get(&format!("border-{side}-width")),
            Some(&ComputedValue::Px(1.0)),
            "textarea border-{side}-width"
        );
        assert_eq!(
            style.get(&format!("border-{side}-style")),
            Some(&ComputedValue::Keyword("solid".to_string()))
        );
        assert_eq!(
            style.get(&format!("padding-{side}")),
            Some(&ComputedValue::Px(2.0)),
            "textarea padding-{side}"
        );
    }
}

#[test]
fn ua_defaults_select_is_inline_block_bordered() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let select = NodeHandle::element("select");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(select.clone());

    let mut resolver = StyleResolver::new();
    let style = resolver.computed_style(&select);
    assert_eq!(
        style.get("display"),
        Some(&ComputedValue::Keyword("inline-block".to_string()))
    );
    assert_eq!(
        style.get("background-color"),
        Some(&ComputedValue::Color("#efefef".to_string()))
    );
    for side in ["top", "right", "bottom", "left"] {
        assert_eq!(
            style.get(&format!("border-{side}-width")),
            Some(&ComputedValue::Px(1.0)),
            "select border-{side}-width"
        );
        assert_eq!(
            style.get(&format!("border-{side}-style")),
            Some(&ComputedValue::Keyword("solid".to_string()))
        );
    }
    assert_eq!(style.get("padding-top"), Some(&ComputedValue::Px(1.0)));
    assert_eq!(style.get("padding-right"), Some(&ComputedValue::Px(4.0)));
    assert_eq!(style.get("padding-bottom"), Some(&ComputedValue::Px(1.0)));
    assert_eq!(style.get("padding-left"), Some(&ComputedValue::Px(4.0)));
}

#[test]
fn animation_forwards_applies_keyframe_final_state() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    div.set_attribute("class", "fade");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "@keyframes fadein { from { opacity: 0; } to { opacity: 1; } } \
             .fade { opacity: 0; animation-name: fadein; animation-fill-mode: forwards; }",
        )
        .unwrap(),
    );
    let style = resolver.computed_style(&div);
    let opacity = style.get("opacity");
    assert!(
        matches!(opacity, Some(ComputedValue::Number(v)) if (*v - 1.0).abs() < 0.01),
        "animation forwards should apply final opacity: 1.0, got {opacity:?}"
    );
}

#[test]
fn animation_fill_mode_none_does_not_apply_keyframe() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    div.set_attribute("class", "fade");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "@keyframes fadein { from { opacity: 0; } to { opacity: 1; } } \
             .fade { opacity: 0; animation-name: fadein; animation-fill-mode: none; }",
        )
        .unwrap(),
    );
    let style = resolver.computed_style(&div);
    let opacity = style.get("opacity");
    assert!(
        matches!(opacity, Some(ComputedValue::Number(v)) if (*v - 0.0).abs() < 0.01),
        "animation fill-mode: none should keep opacity: 0, got {opacity:?}"
    );
}

#[test]
fn animation_does_not_override_inline_important_declaration() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    div.set_attribute("class", "fade");
    div.set_attribute("style", "color: blue !important");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "@keyframes recolor { from { color: black; } to { color: red; } } \
             .fade { animation-name: recolor; animation-fill-mode: forwards; }",
        )
        .unwrap(),
    );

    let style = resolver.computed_style(&div);
    assert_eq!(style.get("color"), Some(&ComputedValue::Color("blue".to_string())));
}

#[test]
fn prefixed_animation_property_does_not_override_canonical_important_declaration() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    div.set_attribute("class", "move");
    div.set_attribute("style", "transform: none !important");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "@keyframes move { to { -webkit-transform: translateX(10px); } } \
             .move { animation-name: move; animation-fill-mode: forwards; }",
        )
        .unwrap(),
    );

    let style = resolver.computed_style(&div);
    assert_eq!(
        style.get("transform"),
        Some(&ComputedValue::Keyword("none".to_string()))
    );
    assert_eq!(style.get("-webkit-transform"), None);
}

#[test]
fn animation_shorthand_forwards_applies_final_state() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    div.set_attribute("class", "fade");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "@keyframes fadein { from { opacity: 0; } to { opacity: 1; } } \
             .fade { opacity: 0; animation: fadein 1s forwards; }",
        )
        .unwrap(),
    );
    let style = resolver.computed_style(&div);
    let opacity = style.get("opacity");
    assert!(
        matches!(opacity, Some(ComputedValue::Number(v)) if (*v - 1.0).abs() < 0.01),
        "animation shorthand with forwards should apply final opacity: 1.0, got {opacity:?}"
    );
}

#[test]
fn infinite_animation_uses_deterministic_visible_snapshot_and_delay() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let visible = NodeHandle::element("div");
    let delayed = NodeHandle::element("div");
    visible.set_attribute("class", "character visible");
    delayed.set_attribute("class", "character delayed");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(visible.clone());
    body.append_child(delayed.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "@keyframes characterFade {
                0% { opacity: 0; }
                5% { opacity: 1; }
                25% { opacity: 1; }
                30%, 100% { opacity: 0; }
             }
             .character { opacity: 0; animation: characterFade 8000ms infinite linear; }
             .delayed { animation-delay: calc(2000ms + 500ms); }",
        )
        .unwrap(),
    );

    assert_eq!(
        resolver.computed_style(&visible).get("opacity"),
        Some(&ComputedValue::Number(1.0))
    );
    assert_eq!(
        resolver.computed_style(&delayed).get("opacity"),
        Some(&ComputedValue::Number(0.0))
    );
    assert_eq!(
        resolver
            .computed_style(&visible)
            .get("animation-iteration-count"),
        Some(&ComputedValue::Keyword("infinite".to_string()))
    );
    assert_eq!(
        resolver.computed_style(&visible).get("animation-duration"),
        Some(&ComputedValue::Number(8.0))
    );
    assert_eq!(
        resolver.computed_style(&delayed).get("animation-delay"),
        Some(&ComputedValue::Number(2.5))
    );
}

#[test]
fn invalid_animation_time_unit_does_not_drive_snapshot() {
    let element = NodeHandle::element("div");
    element.set_attribute("class", "character");
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "@keyframes fade { 0% { opacity: 0; } 100% { opacity: 1; } }
             .character {
                 opacity: 0;
                 animation-name: fade;
                 animation-duration: 8px;
                 animation-iteration-count: infinite;
             }",
        )
        .unwrap(),
    );

    let style = resolver.computed_style(&element);
    assert_eq!(
        style.get("animation-duration"),
        Some(&ComputedValue::Keyword("8px".to_string()))
    );
    assert_eq!(style.get("opacity"), Some(&ComputedValue::Number(0.0)));
}

#[test]
fn ua_defaults_dd_has_margin_left() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let dl = NodeHandle::element("dl");
    let dd = NodeHandle::element("dd");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(dl.clone());
    dl.append_child(dd.clone());

    let mut resolver = StyleResolver::new();
    let style = resolver.computed_style(&dd);
    assert_eq!(style.get("margin-left"), Some(&ComputedValue::Px(40.0)));
}

#[test]
fn ua_defaults_table_has_display_table() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let table = NodeHandle::element("table");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(table.clone());

    let mut resolver = StyleResolver::new();
    let style = resolver.computed_style(&table);
    assert_eq!(style.get("display"), Some(&ComputedValue::Keyword("table".to_string())));
}

#[test]
fn font_size_smaller_resolves_to_px() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let child = NodeHandle::element("span");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(child.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("body { font-size: 20px; } span { font-size: smaller; }").unwrap(),
    );
    let style = resolver.computed_style(&child);
    // smaller = parent * 0.833 = 20 * 0.833 = 16.66
    match style.get("font-size") {
        Some(ComputedValue::Px(px)) => {
            assert!((*px - 16.66).abs() < 0.1, "font-size: smaller should be ~16.66px, got {px}");
        }
        other => panic!("expected Px, got {other:?}"),
    }
}

#[test]
fn font_size_larger_resolves_to_px() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let child = NodeHandle::element("span");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(child.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("body { font-size: 20px; } span { font-size: larger; }").unwrap(),
    );
    let style = resolver.computed_style(&child);
    // larger = parent * 1.2 = 20 * 1.2 = 24.0
    match style.get("font-size") {
        Some(ComputedValue::Px(px)) => {
            assert!((*px - 24.0).abs() < 0.1, "font-size: larger should be ~24px, got {px}");
        }
        other => panic!("expected Px, got {other:?}"),
    }
}

#[test]
fn calc_mixed_percent_and_px_produces_calc_px_percent() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("div { width: calc(100% - 165px); }").unwrap(),
    );
    let style = resolver.computed_style(&div);
    match style.get("width") {
        Some(ComputedValue::CalcPxPercent(px, pct)) => {
            assert!((*px - (-165.0)).abs() < 0.1, "px should be -165, got {px}");
            assert!((*pct - 100.0).abs() < 0.1, "pct should be 100, got {pct}");
        }
        other => panic!("expected CalcPxPercent, got {other:?}"),
    }
}

#[test]
fn outline_shorthand_expands_to_longhands_with_correct_values() {
    use crate::css::{Rule, Value, parse_stylesheet};

    let stylesheet = parse_stylesheet("div { outline: 2px solid red; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };

    let style_decl = rule.declarations.iter().find(|d| d.name == "outline-style")
        .expect("should have outline-style");
    assert_eq!(style_decl.value, Value::Keyword("solid".to_string()));

    let width_decl = rule.declarations.iter().find(|d| d.name == "outline-width")
        .expect("should have outline-width");
    assert_eq!(width_decl.value, Value::Length(2.0, "px".to_string()));

    let color_decl = rule.declarations.iter().find(|d| d.name == "outline-color")
        .expect("should have outline-color");
    assert_eq!(color_decl.value, Value::Keyword("red".to_string()));
}

#[test]
fn outline_none_resets_all_longhands() {
    use crate::css::{Rule, Value, parse_stylesheet};

    let stylesheet = parse_stylesheet("div { outline: none; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };

    let style_decl = rule.declarations.iter().find(|d| d.name == "outline-style")
        .expect("outline: none should produce outline-style");
    assert_eq!(style_decl.value, Value::Keyword("none".to_string()));

    // outline: none should also reset width and color to initial values
    let width_decl = rule.declarations.iter().find(|d| d.name == "outline-width")
        .expect("outline: none should reset outline-width");
    assert_eq!(width_decl.value, Value::Keyword("medium".to_string()));

    let color_decl = rule.declarations.iter().find(|d| d.name == "outline-color")
        .expect("outline: none should reset outline-color");
    assert_eq!(color_decl.value, Value::Keyword("currentcolor".to_string()));
}

#[test]
fn outline_inherit_applies_to_all_longhands() {
    use crate::css::{Rule, Value, parse_stylesheet};

    let stylesheet = parse_stylesheet("div { outline: inherit; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };

    for longhand in ["outline-style", "outline-width", "outline-color"] {
        let decl = rule.declarations.iter().find(|d| d.name == longhand)
            .unwrap_or_else(|| panic!("outline: inherit should produce {longhand}"));
        assert_eq!(decl.value, Value::Keyword("inherit".to_string()),
            "{longhand} should be inherit");
    }
}

#[test]
fn invalid_white_space_keyword_is_dropped_from_cascade() {
    // CSS discards invalid declarations, so a later `white-space: x-bogus`
    // must not override an earlier valid `pre-wrap` (Acid3 test 0 relies on
    // this). The invalid keyword is not a valid `white-space` value.
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let p = NodeHandle::element("p");
    p.set_attribute("id", "target");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(p.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("#target { white-space: pre-wrap; white-space: x-bogus; }").unwrap(),
    );

    let style = resolver.computed_style(&p);
    assert_eq!(
        style.get("white-space"),
        Some(&ComputedValue::Keyword("pre-wrap".to_string())),
        "invalid `x-bogus` must be discarded, keeping `pre-wrap`"
    );
}

#[test]
fn valid_white_space_keyword_survives_cascade() {
    // A valid enumerated value must still win when it is the last declaration.
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let p = NodeHandle::element("p");
    p.set_attribute("id", "target");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(p.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("#target { white-space: pre-wrap; white-space: nowrap; }").unwrap(),
    );

    let style = resolver.computed_style(&p);
    assert_eq!(
        style.get("white-space"),
        Some(&ComputedValue::Keyword("nowrap".to_string())),
        "a valid later value must override the earlier one"
    );
}

#[test]
fn revert_layer_keyword_survives_enumerated_validation() {
    // `revert-layer` (CSS Cascade 5) is a CSS-wide keyword and must be handled
    // exactly like `inherit`/`initial`/`unset`/`revert`: the enumerated-keyword
    // validation must NOT discard it as an invalid `white-space` value. Since it
    // is not dropped, the later `revert-layer` declaration overrides the earlier
    // `pre-wrap` and is preserved as-is (CSS-wide keywords other than `inherit`
    // are not further resolved here).
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let p = NodeHandle::element("p");
    p.set_attribute("id", "target");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(p.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("#target { white-space: pre-wrap; white-space: revert-layer; }").unwrap(),
    );

    let style = resolver.computed_style(&p);
    assert_eq!(
        style.get("white-space"),
        Some(&ComputedValue::Keyword("revert-layer".to_string())),
        "`revert-layer` is CSS-wide and must not be dropped; it overrides `pre-wrap`"
    );
}

/// Builds a `<html><body><p id="target"></p></body></html>` tree and returns
/// the `#target` element for cursor cascade tests.
fn cursor_target_tree() -> (NodeHandle, NodeHandle) {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let p = NodeHandle::element("p");
    p.set_attribute("id", "target");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(p.clone());
    (document, p)
}

/// All `cursor` keywords exercised by Acid3 test 47.
const ACID3_CURSOR_KEYWORDS: &[&str] = &[
    "auto",
    "default",
    "none",
    "context-menu",
    "help",
    "pointer",
    "progress",
    "wait",
    "cell",
    "crosshair",
    "text",
    "vertical-text",
    "alias",
    "copy",
    "move",
    "no-drop",
    "not-allowed",
    "e-resize",
    "n-resize",
    "ne-resize",
    "nw-resize",
    "s-resize",
    "se-resize",
    "sw-resize",
    "w-resize",
    "ew-resize",
    "ns-resize",
    "nesw-resize",
    "nwse-resize",
    "col-resize",
    "row-resize",
    "all-scroll",
];

#[test]
fn invalid_cursor_keyword_falls_back_to_initial_auto() {
    // Acid3 test 47 control case: `cursor: bogus` is not a valid keyword, so the
    // declaration is discarded and computed `cursor` is the initial value `auto`.
    let (_document, target) = cursor_target_tree();

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("#target { cursor: bogus; }").unwrap(),
    );

    let style = resolver.computed_style(&target);
    assert_eq!(
        style.get("cursor"),
        Some(&ComputedValue::Keyword("auto".to_string())),
        "invalid `cursor: bogus` must be dropped, leaving the initial value `auto`"
    );
}

#[test]
fn absent_cursor_defaults_to_initial_auto() {
    // With no `cursor` declaration at all, computed `cursor` is still `auto`.
    let (_document, target) = cursor_target_tree();

    let mut resolver = StyleResolver::new();
    let style = resolver.computed_style(&target);
    assert_eq!(
        style.get("cursor"),
        Some(&ComputedValue::Keyword("auto".to_string())),
        "cursor initial value must be `auto` when undeclared"
    );
}

#[test]
fn all_acid3_cursor_keywords_are_accepted() {
    // Every keyword Acid3 test 47 iterates over must be accepted verbatim.
    for keyword in ACID3_CURSOR_KEYWORDS {
        let (_document, target) = cursor_target_tree();
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(&format!("#target {{ cursor: {keyword}; }}")).unwrap(),
        );
        let style = resolver.computed_style(&target);
        assert_eq!(
            style.get("cursor"),
            Some(&ComputedValue::Keyword(keyword.to_string())),
            "cursor keyword `{keyword}` must be accepted"
        );
    }
}

#[test]
fn valid_cursor_keyword_is_normalized_to_lowercase() {
    // Keywords are ASCII case-insensitive; the computed value is canonicalized
    // to lowercase.
    let (_document, target) = cursor_target_tree();

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("#target { cursor: POINTER; }").unwrap(),
    );

    let style = resolver.computed_style(&target);
    assert_eq!(
        style.get("cursor"),
        Some(&ComputedValue::Keyword("pointer".to_string())),
        "cursor keyword must be normalized to lowercase"
    );
}

#[test]
fn invalid_cursor_does_not_override_earlier_valid() {
    // A later invalid declaration must not clobber an earlier valid one.
    let (_document, target) = cursor_target_tree();

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("#target { cursor: pointer; cursor: bogus; }").unwrap(),
    );

    let style = resolver.computed_style(&target);
    assert_eq!(
        style.get("cursor"),
        Some(&ComputedValue::Keyword("pointer".to_string())),
        "invalid later `cursor: bogus` must be dropped, keeping `pointer`"
    );
}

#[test]
fn invalid_cursor_before_valid_does_not_block() {
    // An earlier invalid declaration must not block a later valid one.
    let (_document, target) = cursor_target_tree();

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("#target { cursor: bogus; cursor: help; }").unwrap(),
    );

    let style = resolver.computed_style(&target);
    assert_eq!(
        style.get("cursor"),
        Some(&ComputedValue::Keyword("help".to_string())),
        "a valid later `cursor: help` must win over an earlier invalid declaration"
    );
}

#[test]
fn valid_cursor_later_declaration_wins() {
    // Two valid declarations: the later one wins per source order.
    let (_document, target) = cursor_target_tree();

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("#target { cursor: pointer; cursor: move; }").unwrap(),
    );

    let style = resolver.computed_style(&target);
    assert_eq!(
        style.get("cursor"),
        Some(&ComputedValue::Keyword("move".to_string())),
        "the later valid `cursor: move` must win"
    );
}

#[test]
fn cursor_url_with_fallback_keyword_is_accepted() {
    // `cursor: url(...), <keyword>` is valid syntax; the trailing keyword is
    // validated and the whole value is retained.
    let (_document, target) = cursor_target_tree();

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("#target { cursor: url(cur.png), pointer; }").unwrap(),
    );

    let style = resolver.computed_style(&target);
    assert_eq!(
        style.get("cursor"),
        Some(&ComputedValue::Keyword("url(cur.png), pointer".to_string())),
        "a url() cursor with a valid fallback keyword must be accepted"
    );
}

#[test]
fn cursor_url_with_invalid_fallback_keyword_is_dropped() {
    // The mandatory trailing keyword is still validated: an invalid fallback
    // makes the whole declaration invalid, so it is dropped back to `auto`.
    let (_document, target) = cursor_target_tree();

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("#target { cursor: url(cur.png), bogus; }").unwrap(),
    );

    let style = resolver.computed_style(&target);
    assert_eq!(
        style.get("cursor"),
        Some(&ComputedValue::Keyword("auto".to_string())),
        "a url() cursor with an invalid fallback keyword must be dropped to `auto`"
    );
}

#[test]
fn cursor_url_without_fallback_keyword_is_dropped() {
    // A url() with no mandatory fallback keyword is invalid per the grammar.
    let (_document, target) = cursor_target_tree();

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("#target { cursor: url(cur.png); }").unwrap(),
    );

    let style = resolver.computed_style(&target);
    assert_eq!(
        style.get("cursor"),
        Some(&ComputedValue::Keyword("auto".to_string())),
        "a url() cursor without a fallback keyword must be dropped to `auto`"
    );
}

#[test]
fn cursor_coordinates_without_url_are_dropped() {
    // `cursor: 1 2 pointer` has coordinates with no preceding `url()`, which
    // violates the grammar, so the whole declaration is dropped back to `auto`.
    let (_document, target) = cursor_target_tree();

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("#target { cursor: 1 2 pointer; }").unwrap(),
    );

    let style = resolver.computed_style(&target);
    assert_eq!(
        style.get("cursor"),
        Some(&ComputedValue::Keyword("auto".to_string())),
        "coordinates with no preceding url() must be dropped to `auto`"
    );
}

#[test]
fn cursor_url_with_single_coordinate_is_dropped() {
    // `cursor: url(cur.png) 1 pointer` has a lone hotspot coordinate; the
    // grammar requires an `<x> <y>` pair, so the declaration is dropped.
    let (_document, target) = cursor_target_tree();

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("#target { cursor: url(cur.png) 1 pointer; }").unwrap(),
    );

    let style = resolver.computed_style(&target);
    assert_eq!(
        style.get("cursor"),
        Some(&ComputedValue::Keyword("auto".to_string())),
        "a url() with a single (unpaired) coordinate must be dropped to `auto`"
    );
}

#[test]
fn cursor_url_with_coordinate_pair_is_accepted() {
    // `cursor: url(cur.png) 1 2, pointer` is valid: the url() carries a hotspot
    // coordinate pair. Serialization keeps the coordinates space-separated and
    // inserts the comma before the trailing keyword.
    let (_document, target) = cursor_target_tree();

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("#target { cursor: url(cur.png) 1 2, pointer; }").unwrap(),
    );

    let style = resolver.computed_style(&target);
    assert_eq!(
        style.get("cursor"),
        Some(&ComputedValue::Keyword("url(cur.png) 1 2, pointer".to_string())),
        "a url() with an `<x> <y>` coordinate pair must be accepted and serialized"
    );
}

#[test]
fn cursor_multiple_url_groups_serialize_with_commas() {
    // `cursor: url(a), url(b), pointer` has two comma-separated url() groups.
    // Each group is serialized separately and joined with commas.
    let (_document, target) = cursor_target_tree();

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("#target { cursor: url(a), url(b), pointer; }").unwrap(),
    );

    let style = resolver.computed_style(&target);
    assert_eq!(
        style.get("cursor"),
        Some(&ComputedValue::Keyword("url(a), url(b), pointer".to_string())),
        "multiple url() groups must be serialized with a comma between each group"
    );
}

#[test]
fn cursor_multiple_url_groups_with_coordinates_serialize_with_commas() {
    // Multiple url() groups, one carrying a coordinate pair, must serialize with
    // group-separating commas while keeping coordinates space-separated.
    let (_document, target) = cursor_target_tree();

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("#target { cursor: url(a), url(b) 1 2, pointer; }").unwrap(),
    );

    let style = resolver.computed_style(&target);
    assert_eq!(
        style.get("cursor"),
        Some(&ComputedValue::Keyword("url(a), url(b) 1 2, pointer".to_string())),
        "coordinate-bearing url() groups must serialize as `url(a), url(b) 1 2, pointer`"
    );
}

#[test]
fn cursor_is_inherited_from_parent() {
    // `cursor` is an inherited property: a child with no cursor declaration
    // takes its parent's computed value.
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let child = NodeHandle::element("span");
    body.set_attribute("id", "parent");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(child.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("#parent { cursor: pointer; }").unwrap(),
    );

    let style = resolver.computed_style(&child);
    assert_eq!(
        style.get("cursor"),
        Some(&ComputedValue::Keyword("pointer".to_string())),
        "cursor must inherit from the parent when the child does not set it"
    );
}

#[test]
fn is_supported_property_includes_cursor() {
    assert!(is_supported_property("cursor"));
}

#[test]
fn inline_cursor_validation_matches_cascade() {
    let computed_value = |style_attribute: &str, property: &str| {
        let element = NodeHandle::element("div");
        element.set_attribute("style", style_attribute);
        StyleResolver::new().computed_style(&element).get(property).cloned()
    };

    assert_eq!(
        computed_value("cursor: pointer", "cursor"),
        Some(ComputedValue::Keyword("pointer".to_string()))
    );
    assert_eq!(
        computed_value("cursor: POINTER", "cursor"),
        Some(ComputedValue::Keyword("pointer".to_string()))
    );
    assert_eq!(
        computed_value("cursor: bogus", "cursor"),
        Some(ComputedValue::Keyword("auto".to_string()))
    );
    assert_eq!(
        computed_value("cursor: url(cur.png), move", "cursor"),
        Some(ComputedValue::Keyword("url(cur.png), move".to_string()))
    );
    assert_eq!(
        computed_value("color: blue", "color"),
        Some(ComputedValue::Color("blue".to_string()))
    );
}

#[test]
fn expands_mask_shorthand_position_size_and_repeat() {
    let stylesheet = parse_stylesheet(
        "h1 { mask: url(mask.svg) 25% 6px / contain no-repeat; }",
    )
    .unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    let declarations: Vec<_> = rule
        .declarations
        .iter()
        .map(|declaration| (declaration.name.as_str(), &declaration.value))
        .collect();

    assert_eq!(
        declarations,
        vec![
            ("mask-image", &Value::Keyword("url(mask.svg)".to_string())),
            ("mask-position-x", &Value::Percentage(25.0)),
            ("mask-position-y", &Value::Length(6.0, "px".to_string())),
            ("mask-size", &Value::Keyword("contain".to_string())),
            ("mask-repeat", &Value::Keyword("no-repeat".to_string())),
        ]
    );
}

#[test]
fn aligns_omitted_mask_shorthand_components_per_layer() {
    let stylesheet = parse_stylesheet("h1 { mask: url(a.svg), url(b.svg) no-repeat; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    let declaration = |name: &str| {
        rule.declarations
            .iter()
            .find(|declaration| declaration.name == name)
            .map(|declaration| &declaration.value)
            .expect("mask shorthand longhand")
    };
    assert_eq!(
        declaration("mask-image"),
        &Value::CommaList(vec![
            Value::Keyword("url(a.svg)".to_string()),
            Value::Keyword("url(b.svg)".to_string()),
        ])
    );
    assert_eq!(
        declaration("mask-repeat"),
        &Value::CommaList(vec![
            Value::Keyword("repeat".to_string()),
            Value::Keyword("no-repeat".to_string()),
        ])
    );

    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, stylesheet);
    let style = resolver.computed_style(&title);
    assert_eq!(
        style.get("mask-repeat"),
        Some(&ComputedValue::Keyword("repeat, no-repeat".to_string()))
    );
}

#[test]
fn canonicalizes_webkit_mask_properties_to_standard_names() {
    let (_document, body, _title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { -webkit-mask-image: url(mask.svg); -webkit-mask-position: 3px 25%; \
             -webkit-mask-size: 8px 4px; -webkit-mask-repeat: no-repeat; }",
        )
        .unwrap(),
    );

    let style = resolver.computed_style(&body);
    assert_eq!(
        style.get("mask-image"),
        Some(&ComputedValue::Keyword("url(mask.svg)".to_string()))
    );
    assert_eq!(style.get("mask-position-x"), Some(&ComputedValue::Px(3.0)));
    assert_eq!(style.get("mask-position-y"), Some(&ComputedValue::Percentage(25.0)));
    assert_eq!(
        style.get("mask-size"),
        Some(&ComputedValue::Keyword("8px 4px".to_string()))
    );
    assert_eq!(
        style.get("mask-repeat"),
        Some(&ComputedValue::Keyword("no-repeat".to_string()))
    );
    for webkit_name in [
        "-webkit-mask-image",
        "-webkit-mask-position",
        "-webkit-mask-size",
        "-webkit-mask-repeat",
    ] {
        assert_eq!(style.get(webkit_name), None);
        assert!(is_supported_property(webkit_name));
    }
}

// --- object-fit / object-position (issue #246) ---

/// Computes the style of an `<img>` carrying `declarations`.
fn image_computed_style(declarations: &str) -> ComputedStyle {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let image = NodeHandle::element("img");
    document.append_child(body.clone());
    body.append_child(image.clone());
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(&format!("img {{ {declarations} }}")).unwrap(),
    );
    resolver.computed_style(&image)
}

fn image_computed_keyword(declarations: &str, property: &str) -> String {
    match image_computed_style(declarations).get(property) {
        Some(ComputedValue::Keyword(keyword)) => keyword.clone(),
        other => panic!("{property} computed to {other:?}"),
    }
}

#[test]
fn background_clip_resolves_initial_unset_and_inherit() {
    assert_eq!(image_computed_keyword("", "background-clip"), "border-box");
    assert_eq!(
        image_computed_keyword("background-clip: CONTENT-BOX", "background-clip"),
        "content-box"
    );
    assert_eq!(
        image_computed_keyword("background-clip: bogus", "background-clip"),
        "border-box"
    );
    for keyword in ["initial", "unset", "revert"] {
        assert_eq!(
            image_computed_keyword(
                &format!("background-clip: {keyword}"),
                "background-clip"
            ),
            "border-box"
        );
    }

    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let parent = NodeHandle::element("div");
    let child = NodeHandle::element("span");
    document.append_child(body.clone());
    body.append_child(parent.clone());
    parent.append_child(child.clone());
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "div { background-clip: content-box; } span { background-clip: inherit; }",
        )
        .unwrap(),
    );
    assert_eq!(
        resolver.computed_style(&child).get("background-clip"),
        Some(&ComputedValue::Keyword("content-box".to_string()))
    );
}

#[test]
fn invalid_conic_angle_syntax_drops_the_whole_declaration() {
    for invalid in [
        "conic-gradient(from to right, red, blue)",
        "conic-gradient(red to right, blue)",
        "conic-gradient(red, to right, blue)",
    ] {
        assert_eq!(
            image_computed_keyword(
                &format!(
                    "background-image: linear-gradient(red, red); background-image: {invalid};"
                ),
                "background-image",
            ),
            "linear-gradient(red, red)",
            "accepted {invalid}",
        );
    }
}

#[test]
fn background_layer_computed_values_preserve_commas_and_repeat_to_image_count() {
    let style = image_computed_style(
        "background-image: linear-gradient(rgb(255, 0, 0), blue), url(a.png), none; \
         background-repeat: no-repeat, repeat; \
         background-position: 1px 2px, 50% 75%; \
         background-size: 2px 3px; \
         background-clip: content-box, padding-box;",
    );
    assert_eq!(
        style.get("background-image"),
        Some(&ComputedValue::Keyword(
            "linear-gradient(rgb(255, 0, 0), blue), url(a.png), none".to_string()
        ))
    );
    assert_eq!(
        style.get("background-repeat"),
        Some(&ComputedValue::Keyword(
            "no-repeat, repeat, no-repeat".to_string()
        ))
    );
    assert_eq!(
        style.get("background-position-x"),
        Some(&ComputedValue::Keyword("1px, 50%, 1px".to_string()))
    );
    assert_eq!(
        style.get("background-position-y"),
        Some(&ComputedValue::Keyword("2px, 75%, 2px".to_string()))
    );
    assert_eq!(
        style.get("background-size"),
        Some(&ComputedValue::Keyword(
            "2px 3px, 2px 3px, 2px 3px".to_string()
        ))
    );
    assert_eq!(
        style.get("background-clip"),
        Some(&ComputedValue::Keyword(
            "content-box, padding-box, content-box".to_string()
        ))
    );
}

#[test]
fn background_layer_lists_truncate_to_a_single_image() {
    let style = image_computed_style(
        "background-image: none; \
         background-position-x: 1px, 2px; \
         background-repeat: no-repeat, repeat; \
         background-origin: content-box, border-box;",
    );
    assert_eq!(
        style.get("background-position-x"),
        Some(&ComputedValue::Keyword("1px".to_string()))
    );
    assert_eq!(
        style.get("background-repeat"),
        Some(&ComputedValue::Keyword("no-repeat".to_string()))
    );
    assert_eq!(
        style.get("background-origin"),
        Some(&ComputedValue::Keyword("content-box".to_string()))
    );

    let style = image_computed_style(
        "background-position-x: 3px, 4px; \
         background-repeat: no-repeat, repeat;",
    );
    assert_eq!(
        style.get("background-position-x"),
        Some(&ComputedValue::Keyword("3px".to_string()))
    );
    assert_eq!(
        style.get("background-repeat"),
        Some(&ComputedValue::Keyword("no-repeat".to_string()))
    );
}

#[test]
fn layered_background_box_keywords_are_normalized_to_lowercase() {
    let style = image_computed_style(
        "background-image: none, none; \
         background-origin: CONTENT-BOX, Padding-Box; \
         background-clip: PADDING-BOX, Border-Box;",
    );
    assert_eq!(
        style.get("background-origin"),
        Some(&ComputedValue::Keyword(
            "content-box, padding-box".to_string()
        ))
    );
    assert_eq!(
        style.get("background-clip"),
        Some(&ComputedValue::Keyword(
            "padding-box, border-box".to_string()
        ))
    );
}

#[test]
fn background_shorthand_accepts_all_named_colors_case_insensitively() {
    assert_eq!(
        image_computed_style("background: PiNk").get("background-color"),
        Some(&ComputedValue::Color("PiNk".to_string()))
    );
    assert_eq!(
        image_computed_style("background: REBECCAPURPLE").get("background-color"),
        Some(&ComputedValue::Color("REBECCAPURPLE".to_string()))
    );
}

#[test]
fn background_layer_computed_values_resolve_each_layers_relative_units() {
    let style = image_computed_style(
        "background-image: none, none; \
         background-position-x: 1em, calc(1em + 2px); \
         background-size: 2em 3em, calc(1em + 4px) 50%;",
    );
    assert_eq!(
        style.get("background-position-x"),
        Some(&ComputedValue::Keyword("16px, 18px".to_string()))
    );
    assert_eq!(
        style.get("background-size"),
        Some(&ComputedValue::Keyword(
            "32px 48px, 20px 50%".to_string()
        ))
    );
}

#[test]
fn single_layer_two_axis_background_repeat_keeps_both_values() {
    let style = image_computed_style(
        "background-image: none; background-repeat: repeat no-repeat;",
    );
    assert_eq!(
        style.get("background-repeat"),
        Some(&ComputedValue::Keyword("repeat no-repeat".to_string()))
    );
}

#[test]
fn background_shorthand_size_accepts_computed_math_functions_per_layer() {
    let style = image_computed_style(
        "background: none 0 0 / calc(1em + 2px) auto, \
         none 0 0 / clamp(1px, 2px, 3px) auto;",
    );
    assert_eq!(
        style.get("background-size"),
        Some(&ComputedValue::Keyword(
            "18px auto, 2px auto".to_string()
        ))
    );
}

#[test]
fn malformed_background_image_layer_drops_the_whole_declaration() {
    let style = image_computed_style(
        "background-image: url(valid.png); \
         background-image: radial-gradient(circle at, red, blue), url(other.png);",
    );
    assert_eq!(
        style.get("background-image"),
        Some(&ComputedValue::Keyword("url(valid.png)".to_string()))
    );
}

#[test]
fn malformed_background_longhand_layer_drops_the_whole_declaration() {
    let style = image_computed_style(
        "background-image: url(valid.png); background-image: url(other.png), bogus; \
         background-repeat: no-repeat; background-repeat: repeat-x, bogus; \
         background-attachment: fixed; background-attachment: scroll, sideways; \
         background-origin: content-box; background-origin: padding-box, margin-box; \
         background-clip: padding-box; background-clip: border-box, margin-box;",
    );
    assert_eq!(
        style.get("background-image"),
        Some(&ComputedValue::Keyword("url(valid.png)".to_string()))
    );
    assert_eq!(
        style.get("background-repeat"),
        Some(&ComputedValue::Keyword("no-repeat".to_string()))
    );
    assert_eq!(
        style.get("background-attachment"),
        Some(&ComputedValue::Keyword("fixed".to_string()))
    );
    assert_eq!(
        style.get("background-origin"),
        Some(&ComputedValue::Keyword("content-box".to_string()))
    );
    assert_eq!(
        style.get("background-clip"),
        Some(&ComputedValue::Keyword("padding-box".to_string()))
    );
}

#[test]
fn single_axis_background_position_keywords_center_the_other_axis() {
    let style = image_computed_style(
        "background-image: none, none; background-position: top, left;",
    );
    assert_eq!(
        style.get("background-position-x"),
        Some(&ComputedValue::Keyword("center, left".to_string()))
    );
    assert_eq!(
        style.get("background-position-y"),
        Some(&ComputedValue::Keyword("top, center".to_string()))
    );
}

#[test]
fn background_position_keyword_pairs_normalize_axis_order() {
    let positions = "left top, top left, right bottom, bottom right, \
                     left center, center left, right center, center right, \
                     top center, center top, bottom center, center bottom, center center";
    let images = std::iter::repeat("none")
        .take(13)
        .collect::<Vec<_>>()
        .join(", ");

    for declarations in [
        format!("background-image: {images}; background-position: {positions};"),
        format!("background: {positions}; background-image: {images};"),
    ] {
        let style = image_computed_style(&declarations);
        assert_eq!(
            style.get("background-position-x"),
            Some(&ComputedValue::Keyword(
                "left, left, right, right, left, left, right, right, center, center, center, center, center".to_string()
            )),
            "{declarations}",
        );
        assert_eq!(
            style.get("background-position-y"),
            Some(&ComputedValue::Keyword(
                "top, top, bottom, bottom, center, center, center, center, top, top, bottom, bottom, center".to_string()
            )),
            "{declarations}",
        );
    }
}

#[test]
fn background_position_rejects_two_keywords_from_the_same_axis() {
    for invalid in [
        "left right",
        "right left",
        "left left",
        "right right",
        "top bottom",
        "bottom top",
        "top top",
        "bottom bottom",
    ] {
        let style = image_computed_style(&format!(
            "background-position: 3px 4px; background-position: {invalid};"
        ));
        assert_eq!(
            style.get("background-position-x"),
            Some(&ComputedValue::Px(3.0)),
            "accepted background-position: {invalid}",
        );
        assert_eq!(
            style.get("background-position-y"),
            Some(&ComputedValue::Px(4.0)),
            "accepted background-position: {invalid}",
        );

        let style = image_computed_style(&format!(
            "background: none 3px 4px; background: none {invalid};"
        ));
        assert_eq!(
            style.get("background-position-x"),
            Some(&ComputedValue::Px(3.0)),
            "accepted background layer position: {invalid}",
        );
        assert_eq!(
            style.get("background-position-y"),
            Some(&ComputedValue::Px(4.0)),
            "accepted background layer position: {invalid}",
        );
    }
}

#[test]
fn background_position_keyword_and_length_pairs_obey_axis_slots() {
    for declarations in [
        "background-image: none, none; background-position: 10px top, left 20px;",
        "background: none 10px top, none left 20px;",
    ] {
        let style = image_computed_style(declarations);
        assert_eq!(
            style.get("background-position-x"),
            Some(&ComputedValue::Keyword("10px, left".to_string())),
            "{declarations}",
        );
        assert_eq!(
            style.get("background-position-y"),
            Some(&ComputedValue::Keyword("top, 20px".to_string())),
            "{declarations}",
        );
    }

    for invalid in ["top 10px", "10px left"] {
        for declarations in [
            format!("background-position: 3px 4px; background-position: {invalid};"),
            format!("background: none 3px 4px; background: none {invalid};"),
        ] {
            let style = image_computed_style(&declarations);
            assert_eq!(
                style.get("background-position-x"),
                Some(&ComputedValue::Px(3.0)),
                "accepted {declarations}",
            );
            assert_eq!(
                style.get("background-position-y"),
                Some(&ComputedValue::Px(4.0)),
                "accepted {declarations}",
            );
        }
    }
}

#[test]
fn background_size_slash_requires_a_preceding_position_and_rejects_later_positions() {
    for invalid in [
        "none / 2px 2px",
        "none / 2px 2px left top",
        "none left / 2px 2px top",
    ] {
        let style = image_computed_style(&format!(
            "background: none 3px 4px / 5px 6px no-repeat; background: {invalid};"
        ));
        assert_eq!(
            style.get("background-position-x"),
            Some(&ComputedValue::Px(3.0)),
            "accepted background: {invalid}",
        );
        assert_eq!(
            style.get("background-position-y"),
            Some(&ComputedValue::Px(4.0)),
            "accepted background: {invalid}",
        );
        assert_eq!(
            style.get("background-size"),
            Some(&ComputedValue::Keyword("5px 6px".to_string())),
            "accepted background: {invalid}",
        );
    }
}

/// Both properties are exposed with their initial values even when nothing
/// declares them, so getComputedStyle can serialize them (Firefox 152: `fill`
/// and `50% 50%`).
#[test]
fn object_fit_and_position_expose_their_initial_values() {
    assert_eq!(image_computed_keyword("", "object-fit"), "fill");
    assert_eq!(image_computed_keyword("", "object-position"), "50% 50%");
    assert!(is_supported_property("object-fit"));
    assert!(is_supported_property("object-position"));
}

#[test]
fn object_fit_accepts_the_css_images_keywords() {
    for keyword in ["fill", "contain", "cover", "none", "scale-down"] {
        assert_eq!(
            image_computed_keyword(&format!("object-fit: {keyword}"), "object-fit"),
            keyword
        );
    }
    // Keywords are ASCII case-insensitive.
    assert_eq!(image_computed_keyword("object-fit: SCALE-DOWN", "object-fit"), "scale-down");
}

/// An invalid declaration is dropped, so the initial value stays in effect and
/// an earlier valid declaration still wins (Firefox 152 reports `fill` for all
/// of these).
#[test]
fn invalid_object_fit_declarations_are_dropped() {
    for value in ["bogus", "fill fill", "50%", "10px", "none cover"] {
        assert_eq!(
            image_computed_keyword(&format!("object-fit: {value}"), "object-fit"),
            "fill",
            "object-fit: {value} must be dropped"
        );
    }
    assert_eq!(
        image_computed_keyword("object-fit: cover; object-fit: bogus", "object-fit"),
        "cover",
        "a dropped declaration must not clobber the earlier valid one"
    );
    for keyword in ["initial", "unset"] {
        assert_eq!(
            image_computed_keyword(&format!("object-fit: {keyword}"), "object-fit"),
            "fill"
        );
    }
}

/// `object-position` computes to two components in `x y` order, with keywords
/// turned into percentages and lengths into pixels. Every expectation below was
/// read out of Firefox 152 over Marionette.
#[test]
fn object_position_computes_to_normalized_x_y_components() {
    for (declared, expected) in [
        ("left", "0% 50%"),
        ("right", "100% 50%"),
        ("top", "50% 0%"),
        ("bottom", "50% 100%"),
        ("center", "50% 50%"),
        ("left top", "0% 0%"),
        ("right bottom", "100% 100%"),
        ("50%", "50% 50%"),
        ("25% 75%", "25% 75%"),
        ("10px", "10px 50%"),
        ("10px 20px", "10px 20px"),
        ("center top", "50% 0%"),
        // A leading `top` names the vertical axis, so the components swap.
        ("top center", "50% 0%"),
        ("bottom right", "100% 100%"),
        ("0 0", "0px 0px"),
        ("0% 0%", "0% 0%"),
        ("left 10px", "0% 10px"),
        ("10px center", "10px 50%"),
        ("-10% 110%", "-10% 110%"),
        // Lengths resolve against the used font size (16px) and viewport.
        ("2em", "32px 50%"),
        ("calc(10px + 5px) 50%", "15px 50%"),
    ] {
        assert_eq!(
            image_computed_keyword(&format!("object-position: {declared}"), "object-position"),
            expected,
            "object-position: {declared}"
        );
    }
}

#[test]
fn invalid_object_position_declarations_are_dropped() {
    for value in [
        "bogus",
        "left right",
        "top bottom",
        "top 10px left",
        "10px 20px 30px",
        "center center center",
    ] {
        assert_eq!(
            image_computed_keyword(&format!("object-position: {value}"), "object-position"),
            "50% 50%",
            "object-position: {value} must be dropped"
        );
    }
    assert_eq!(
        image_computed_keyword(
            "object-position: 10px 20px; object-position: bogus",
            "object-position"
        ),
        "10px 20px"
    );
    for keyword in ["initial", "unset"] {
        assert_eq!(
            image_computed_keyword(&format!("object-position: {keyword}"), "object-position"),
            "50% 50%"
        );
    }
}

/// A CSS-wide keyword must reach the cascade instead of being rejected by the
/// property grammar. Firefox 152 with a parent at `10px 20px` / `cover`:
/// `inherit` copies the parent, and `initial` / `unset` / `revert` fall back to
/// the initial value.
#[test]
fn object_fit_and_position_resolve_css_wide_keywords() {
    let inherited = |child_declarations: &str, property: &str| {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let parent = NodeHandle::element("div");
        let image = NodeHandle::element("img");
        document.append_child(body.clone());
        body.append_child(parent.clone());
        parent.append_child(image.clone());
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(&format!(
                "div {{ object-fit: cover; object-position: 10px 20px; }} \
                 img {{ {child_declarations} }}"
            ))
            .unwrap(),
        );
        match resolver.computed_style(&image).get(property) {
            Some(ComputedValue::Keyword(keyword)) => keyword.clone(),
            other => panic!("{property} computed to {other:?}"),
        }
    };

    assert_eq!(inherited("object-position: inherit", "object-position"), "10px 20px");
    assert_eq!(inherited("object-fit: inherit", "object-fit"), "cover");
    for keyword in ["initial", "unset", "revert"] {
        assert_eq!(
            inherited(&format!("object-position: {keyword}"), "object-position"),
            "50% 50%"
        );
        assert_eq!(
            inherited(&format!("object-fit: {keyword}"), "object-fit"),
            "fill"
        );
    }
    // Without a CSS-wide keyword neither property inherits.
    assert_eq!(inherited("", "object-position"), "50% 50%");
    assert_eq!(inherited("", "object-fit"), "fill");
}

/// `calc()` is only a valid position component when it carries a length or a
/// percentage. Firefox 152 drops `calc(1)`, `calc(1 + 2)` and `calc(0)`.
#[test]
fn object_position_rejects_calc_without_a_length_or_percentage() {
    for value in ["calc(1)", "calc(1 + 2)", "calc(0)", "calc(2 * 3) 50%"] {
        assert_eq!(
            image_computed_keyword(&format!("object-position: {value}"), "object-position"),
            "50% 50%",
            "object-position: {value} must be dropped"
        );
    }
    for (declared, expected) in [
        ("calc(10px + 5px)", "15px 50%"),
        ("calc(2em * 2)", "64px 50%"),
        ("calc(10px) calc(20%)", "10px 20%"),
        // A mixed-unit calc cannot collapse to one unit, so it survives as an
        // expression — exactly what Firefox 152 serializes.
        ("calc(50% - 10px)", "calc(50% - 10px) 50%"),
    ] {
        assert_eq!(
            image_computed_keyword(&format!("object-position: {declared}"), "object-position"),
            expected,
            "object-position: {declared}"
        );
    }
}

// --- aspect-ratio (issue #247) ---

/// `aspect-ratio` computes to `auto`, a normalized `W / H` ratio, or both.
/// Every expectation was read out of Firefox 152 over Marionette.
#[test]
fn aspect_ratio_computes_to_a_normalized_ratio() {
    assert_eq!(image_computed_keyword("", "aspect-ratio"), "auto");
    assert!(is_supported_property("aspect-ratio"));

    for (declared, expected) in [
        ("auto", "auto"),
        ("1/1", "1 / 1"),
        ("2 / 1", "2 / 1"),
        // A lone number is a ratio against 1.
        ("2", "2 / 1"),
        ("0.5", "0.5 / 1"),
        ("1/3", "1 / 3"),
        // `auto` always serializes first, whichever order it was written in.
        ("auto 2/1", "auto 2 / 1"),
        ("2/1 auto", "auto 2 / 1"),
        ("auto 2", "auto 2 / 1"),
        // A degenerate ratio is still a valid computed value; layout is what
        // ignores it.
        ("0/1", "0 / 1"),
        ("1/0", "1 / 0"),
        ("0/0", "0 / 0"),
        ("calc(1) / calc(2)", "1 / 2"),
        ("calc(2 * 3)", "6 / 1"),
    ] {
        assert_eq!(
            image_computed_keyword(&format!("aspect-ratio: {declared}"), "aspect-ratio"),
            expected,
            "aspect-ratio: {declared}"
        );
    }
}

#[test]
fn invalid_aspect_ratio_declarations_are_dropped() {
    for value in [
        "-1/1",
        "1/-1",
        "1 1",
        "a/b",
        "1/",
        "/1",
        "1//2",
        "auto auto",
        "auto auto 1/1",
        "1/1 1/1",
        "1/1 auto 1/1",
        "10px/1",
        // A percentage or length cannot be a ratio component, unlike an
        // out-of-range number (see out_of_range_calc_aspect_ratio_is_clamped_not_dropped).
        "calc(50%) / 1",
        "calc(1em) / 1",
    ] {
        assert_eq!(
            image_computed_keyword(&format!("aspect-ratio: {value}"), "aspect-ratio"),
            "auto",
            "aspect-ratio: {value} must be dropped"
        );
    }
    assert_eq!(
        image_computed_keyword("aspect-ratio: 3/2; aspect-ratio: bogus", "aspect-ratio"),
        "3 / 2",
        "a dropped declaration must not clobber the earlier valid one"
    );
}

#[test]
fn aspect_ratio_resolves_css_wide_keywords() {
    let inherited = |child_declarations: &str| {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let parent = NodeHandle::element("div");
        let image = NodeHandle::element("img");
        document.append_child(body.clone());
        body.append_child(parent.clone());
        parent.append_child(image.clone());
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(&format!(
                "div {{ aspect-ratio: 3 / 2; }} img {{ {child_declarations} }}"
            ))
            .unwrap(),
        );
        match resolver.computed_style(&image).get("aspect-ratio") {
            Some(ComputedValue::Keyword(keyword)) => keyword.clone(),
            other => panic!("aspect-ratio computed to {other:?}"),
        }
    };

    assert_eq!(inherited("aspect-ratio: inherit"), "3 / 2");
    for keyword in ["initial", "unset", "revert"] {
        assert_eq!(inherited(&format!("aspect-ratio: {keyword}")), "auto");
    }
    // It is not an inherited property.
    assert_eq!(inherited(""), "auto");
}

/// A `calc()` that resolves outside `<number [0,∞]>` is clamped rather than
/// dropped (CSS Values 4), so it still replaces an earlier declaration. Firefox
/// 152 reports the same clamped values.
#[test]
fn out_of_range_calc_aspect_ratio_is_clamped_not_dropped() {
    for (declared, expected) in [
        ("calc(-1)", "0 / 1"),
        ("calc(-1) / 2", "0 / 2"),
        ("2 / calc(-1)", "2 / 0"),
        ("calc(1 + 1)/calc(4)", "2 / 4"),
    ] {
        assert_eq!(
            image_computed_keyword(&format!("aspect-ratio: {declared}"), "aspect-ratio"),
            expected,
            "aspect-ratio: {declared}"
        );
    }

    // A clamped value is valid, so unlike a dropped one it wins the cascade.
    assert_eq!(
        image_computed_keyword("aspect-ratio: 3/2; aspect-ratio: calc(-1)", "aspect-ratio"),
        "0 / 1"
    );
    assert_eq!(
        image_computed_keyword("aspect-ratio: 3/2; aspect-ratio: bogus", "aspect-ratio"),
        "3 / 2"
    );

    // Values that overflow the float range are dropped here, because the calc
    // evaluator reports no result for them. Firefox 152 saturates at the largest
    // float instead (`3.40282e38 / 1`).
    for value in ["1e40", "calc(1e40)", "calc(1/0)", "calc(0/0)"] {
        assert_eq!(
            image_computed_keyword(&format!("aspect-ratio: {value}"), "aspect-ratio"),
            "auto",
            "aspect-ratio: {value}"
        );
    }
}
