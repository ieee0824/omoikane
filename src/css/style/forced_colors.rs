//! Immutable paint snapshots using the host's owned forced-color palette.
use super::{
    ComputedStyle, ComputedValue, DeclarationValidation, Value, is_css_wide_keyword, render_value,
};
use crate::css::{ForcedColorPalette, MediaEnvironment};
use crate::dom::{Node, NodeHandle, NodeType};
use std::collections::BTreeMap;
use std::sync::Arc;

pub(super) fn validate(name: &str, value: &Value) -> Option<DeclarationValidation> {
    if name != "forced-color-adjust" {
        return None;
    }
    let valid = matches!(value, Value::Keyword(keyword) if
        matches!(keyword.to_ascii_lowercase().as_str(), "auto" | "none" | "preserve-parent-color")
        || is_css_wide_keyword(&keyword.to_ascii_lowercase()));
    Some(if valid {
        DeclarationValidation::Valid(ComputedValue::Keyword(render_value(value)))
    } else {
        DeclarationValidation::Invalid
    })
}

/// Namespace-qualified UA defaults protect SVG artwork while allowing HTML
/// inside foreignObject to resume the surrounding forced-color presentation.
pub(super) fn apply_svg_ua_defaults(node: &NodeHandle, properties: &mut super::PropertyMap) {
    if node.namespace_uri().as_deref() != Some("http://www.w3.org/2000/svg") {
        return;
    }
    let value = match node.local_name().as_deref() {
        Some("svg") => "preserve-parent-color",
        Some("foreignObject") => "auto",
        _ => return,
    };
    if !properties.contains_key("forced-color-adjust") {
        properties.insert("forced-color-adjust", ComputedValue::Keyword(value.into()));
    }
}

/// Resolves explicit system colors before currentcolor and inheritance.
/// Opting out of automatic forcing does not opt out of system color values.
pub(super) fn resolve_system_colors(
    properties: &mut super::PropertyMap,
    environment: &MediaEnvironment,
) {
    let palette = environment.used_forced_color_palette().unwrap_or_else(|| {
        ForcedColorPalette::for_color_scheme(environment.preferred_color_scheme_dark())
    });
    let replacements: Vec<_> = properties
        .iter()
        .filter_map(|(name, value)| {
            if !super::is_color_property(name)
                && !matches!(name, "caret-color" | "accent-color" | "fill" | "stroke")
            {
                return None;
            }
            let system_name = value.css_text();
            let rgb = palette.system_color(&system_name)?;
            Some((
                name.to_string(),
                ComputedValue::Color(format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])),
                system_name,
            ))
        })
        .collect();
    for (name, value, system_name) in replacements {
        properties.insert(&name, value);
        properties.mark_system_color(&name, &system_name);
    }
}

/// Captures the winning color declaration before CSS-wide keywords resolve.
/// Plain color components are temporary; only layout-dependent ones survive.
pub(super) fn capture_color_inheritance(
    values: &mut BTreeMap<String, Value>,
    properties: &super::PropertyMap,
) -> bool {
    let raw = values.remove("color");
    let inherits = match raw.as_ref() {
        Some(Value::Keyword(keyword) | Value::Color(keyword)) => matches!(
            keyword.to_ascii_lowercase().as_str(),
            "inherit" | "unset" | "currentcolor"
        ),
        None => properties.get("color").is_none(),
        _ => false,
    };
    if let Some(value) = raw.filter(super::color_uses_container_units) {
        values.insert("color".into(), value);
    }
    inherits
}

/// This exception changes computed color, using the unvisited parent's used
/// color. The parent's ordinary cache and all other child properties stay intact.
pub(super) fn preserve_parent_color(
    node: &NodeHandle,
    pseudo: bool,
    parent_style: Option<&ComputedStyle>,
    properties: &mut super::PropertyMap,
    inherits: bool,
    environment: &MediaEnvironment,
) {
    if !inherits
        || environment.used_forced_color_palette().is_none()
        || !properties.get("forced-color-adjust").is_some_and(|value| {
            value
                .css_text()
                .eq_ignore_ascii_case("preserve-parent-color")
        })
    {
        return;
    }
    let Some(parent_style) = parent_style else {
        return;
    };
    let parent = if pseudo {
        Some(node.clone())
    } else {
        super::flattened_assigned_slot(node)
            .or_else(|| node.parent_node())
            .map(|parent| {
                if parent.node_type() == NodeType::DocumentFragment {
                    parent.shadow_host().unwrap_or(parent)
                } else {
                    parent
                }
            })
    };
    let Some(parent) = parent else {
        return;
    };
    let used = paint_style(&parent, Arc::new(parent_style.clone()), environment);
    properties.insert_from("color", &used.properties);
}

/// Applies the computed-value exceptions after animation and transitions.
pub(super) fn compute_presentation_adjustments(
    properties: &mut super::PropertyMap,
    components: &mut BTreeMap<String, Value>,
    environment: &MediaEnvironment,
) {
    if environment.used_forced_color_palette().is_none()
        || properties
            .get("forced-color-adjust")
            .is_some_and(|value| !value.css_text().eq_ignore_ascii_case("auto"))
    {
        return;
    }
    for (name, value) in [
        ("box-shadow", "none"),
        ("text-shadow", "none"),
        ("color-scheme", "light dark"),
        ("scrollbar-color", "auto"),
        ("accent-color", "auto"),
    ] {
        properties.insert(name, ComputedValue::Keyword(value.into()));
        components.remove(name);
    }
    let contains_url = properties.get("background-image").is_some_and(|value| {
        crate::css::parse_style_attribute(&format!("background-image:{}", value.css_text()))
            .iter()
            .any(|declaration| value_contains_url(&declaration.value))
    });
    if !contains_url {
        properties.insert("background-image", ComputedValue::Keyword("none".into()));
        components.remove("background-image");
    }
}

fn value_contains_url(value: &Value) -> bool {
    match value {
        Value::Function { name, arguments } => {
            name.eq_ignore_ascii_case("url") || arguments.iter().any(value_contains_url)
        }
        // The parser retains the spelling of url() as one keyword token.
        Value::Keyword(keyword) => keyword.to_ascii_lowercase().starts_with("url("),
        Value::List(values) | Value::CommaList(values) => values.iter().any(value_contains_url),
        _ => false,
    }
}

pub(super) fn paint_style(
    node: &NodeHandle,
    ordinary: Arc<ComputedStyle>,
    environment: &MediaEnvironment,
) -> Arc<ComputedStyle> {
    let Some(palette) = environment.used_forced_color_palette() else {
        return ordinary;
    };
    let adjust = ordinary
        .get("forced-color-adjust")
        .map(ComputedValue::css_text)
        .unwrap_or_default();
    if adjust.eq_ignore_ascii_case("none") || adjust.eq_ignore_ascii_case("preserve-parent-color") {
        return ordinary;
    }
    let mut paint = ordinary.as_ref().clone();
    let (foreground, background) = semantic_colors(node, palette);
    for name in [
        "color",
        "border-top-color",
        "border-right-color",
        "border-bottom-color",
        "border-left-color",
        "outline-color",
        "column-rule-color",
        "text-decoration-color",
        "caret-color",
    ] {
        force_color(&mut paint, name, foreground);
    }
    let background = paint
        .properties
        .system_color_name("color")
        .and_then(|name| paired_background(palette, name))
        .unwrap_or(background);
    force_color(&mut paint, "background-color", background);
    for name in ["box-shadow", "text-shadow"] {
        paint
            .properties
            .insert(name, ComputedValue::Keyword("none".into()));
    }
    Arc::new(paint)
}

fn force_color(style: &mut ComputedStyle, name: &str, rgb: [u8; 3]) {
    if style.properties.system_color_name(name).is_some() {
        return;
    }
    if style
        .get(name)
        .is_some_and(|value| value.css_text().eq_ignore_ascii_case("currentcolor"))
        && style.properties.system_color_name("color").is_some()
    {
        style.properties.copy_property("color", name);
        return;
    }
    let alpha = style
        .get(name)
        .and_then(|value| crate::paint::color::parse_color(&value.css_text()))
        .map_or(255, |color| color.a);
    style.properties.insert(
        name,
        ComputedValue::Color(format!(
            "#{:02x}{:02x}{:02x}{:02x}",
            rgb[0], rgb[1], rgb[2], alpha
        )),
    );
}

/// Chooses the opposite of an explicit foreground system color, retaining
/// keyword identity even when several palette entries contain equal RGB.
fn paired_background(palette: ForcedColorPalette, name: &str) -> Option<[u8; 3]> {
    let opposite = match name.to_ascii_lowercase().as_str() {
        "canvas" => "canvastext",
        "canvastext" | "linktext" | "visitedtext" | "activetext" => "canvas",
        "buttontext" => "buttonface",
        "buttonface" => "buttontext",
        "fieldtext" => "field",
        "field" => "fieldtext",
        "highlighttext" => "highlight",
        "highlight" => "highlighttext",
        "marktext" => "mark",
        "mark" => "marktext",
        "selecteditemtext" => "selecteditem",
        "selecteditem" => "selecteditemtext",
        "accentcolortext" => "accentcolor",
        "accentcolor" => "accentcolortext",
        // GrayText can contrast with any background, and ButtonBorder has
        // several valid adjacent colors. Keep the element's semantic choice.
        _ => return None,
    };
    palette.system_color(opposite)
}

fn semantic_colors(node: &NodeHandle, palette: ForcedColorPalette) -> ([u8; 3], [u8; 3]) {
    let mut current = Some(node.clone());
    while let Some(node) = current {
        match node.tag_name().as_deref() {
            Some("a" | "area") if node.get_attribute("href").is_some() => {
                return (palette.link_text, palette.canvas);
            }
            Some("button") => return (palette.button_text, palette.button_face),
            Some("input" | "textarea" | "select") => return (palette.field_text, palette.field),
            _ => {}
        }
        current = node.parent_node();
    }
    (palette.canvas_text, palette.canvas)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::StyleResolver;
    use crate::html::TreeBuilder;

    fn active_resolver() -> crate::css::StyleResolver {
        let mut resolver = crate::css::StyleResolver::new();
        let mut environment = MediaEnvironment::default();
        environment.set_feature("forced-colors", "active");
        let mut palette = ForcedColorPalette::for_color_scheme(false);
        palette.highlight = [11, 22, 33];
        environment.forced_color_palette = Some(palette);
        resolver.set_media_environment(environment);
        resolver
    }

    fn rgb(style: &ComputedStyle, property: &str) -> crate::paint::color::Color {
        crate::paint::color::parse_color(&style.get(property).unwrap().css_text()).unwrap()
    }

    #[test]
    fn svg_ua_colors_preserve_illustrations_and_foreign_object_resumes_forcing() {
        let document = TreeBuilder::parse("<div style='color:red'><svg id='svg' style='background:red'><g id='graphic' style='background:red'></g><foreignObject id='foreign' style='background:red'><div id='html' style='background:red'></div></foreignObject></svg></div>").document();
        let mut nodes = std::collections::BTreeMap::new();
        let mut pending = vec![document.clone()];
        while let Some(node) = pending.pop() {
            if let Some(id) = node.get_attribute("id") {
                nodes.insert(id, node.clone());
            }
            pending.extend(node.child_nodes());
        }
        assert_eq!(
            nodes["svg"].namespace_uri().as_deref(),
            Some("http://www.w3.org/2000/svg")
        );
        assert_eq!(
            nodes["foreign"].local_name().as_deref(),
            Some("foreignObject")
        );
        let mut resolver = active_resolver();
        for id in ["svg", "graphic"] {
            let ordinary = resolver.computed_style(&nodes[id]);
            assert_eq!(
                ordinary.get("forced-color-adjust").unwrap().css_text(),
                "preserve-parent-color",
                "{id}"
            );
            assert_eq!(
                rgb(&ordinary, "color"),
                crate::paint::color::Color::rgb(0, 0, 0)
            );
            assert_eq!(
                rgb(&resolver.paint_style(&nodes[id]), "background-color"),
                crate::paint::color::Color::rgb(255, 0, 0)
            );
        }
        for id in ["foreign", "html"] {
            let ordinary = resolver.computed_style(&nodes[id]);
            assert_eq!(
                ordinary.get("forced-color-adjust").unwrap().css_text(),
                "auto",
                "{id}"
            );
            assert_eq!(
                rgb(&resolver.paint_style(&nodes[id]), "background-color"),
                crate::paint::color::Color::rgb(255, 255, 255)
            );
        }
    }

    #[test]
    fn svg_ua_defaults_respect_namespaces_local_names_and_author_cascade() {
        let namespace = "http://www.w3.org/2000/svg";
        for (tag, ns, author, expected) in [
            ("p:svg", Some(namespace), "", "preserve-parent-color"),
            ("svg", Some("urn:other"), "", "auto"),
            ("svg", None, "", "auto"),
            ("svg", Some(namespace), "forced-color-adjust:auto", "auto"),
            ("svg", Some(namespace), "forced-color-adjust:none", "none"),
        ] {
            let document = crate::dom::NodeHandle::document();
            let node = crate::dom::NodeHandle::xml_element(tag, ns.map(str::to_owned));
            node.set_attribute("style", author);
            document.append_child(node.clone());
            let mut resolver = active_resolver();
            assert_eq!(
                resolver
                    .computed_style(&node)
                    .get("forced-color-adjust")
                    .unwrap()
                    .css_text(),
                expected,
                "{tag} {ns:?} {author}"
            );
        }
        for (tag, author, expected) in [
            ("foreignObject", "", "auto"),
            ("foreignobject", "", "preserve-parent-color"),
            ("foreignObject", "forced-color-adjust:none", "none"),
            (
                "foreignObject",
                "forced-color-adjust:inherit",
                "preserve-parent-color",
            ),
        ] {
            let document = crate::dom::NodeHandle::document();
            let svg = crate::dom::NodeHandle::xml_element("svg", Some(namespace.into()));
            let node = crate::dom::NodeHandle::xml_element(tag, Some(namespace.into()));
            node.set_attribute("style", author);
            svg.append_child(node.clone());
            document.append_child(svg);
            let mut resolver = active_resolver();
            assert_eq!(
                resolver
                    .computed_style(&node)
                    .get("forced-color-adjust")
                    .unwrap()
                    .css_text(),
                expected,
                "{tag} {author}"
            );
        }
    }

    #[test]
    fn paired_background_follows_inherited_color_alpha_opt_out_and_reset() {
        let document = crate::dom::NodeHandle::document();
        let parent = crate::dom::NodeHandle::element("div");
        parent.set_attribute("style", "color:ButtonText");
        let child = crate::dom::NodeHandle::element("span");
        child.set_attribute("style", "background-color:rgba(255,0,0,0.5)");
        parent.append_child(child.clone());
        document.append_child(parent.clone());
        let mut environment = MediaEnvironment::default();
        environment.set_feature("forced-colors", "active");
        let mut palette = ForcedColorPalette::for_color_scheme(false);
        palette.button_face = [11, 22, 33];
        palette.field = [44, 55, 66];
        environment.forced_color_palette = Some(palette);
        let mut resolver = crate::css::StyleResolver::new();
        resolver.set_media_environment(environment.clone());
        let ordinary = resolver.computed_style(&child);
        let alpha = rgb(&ordinary, "background-color").a;
        assert_eq!(
            rgb(&resolver.paint_style(&child), "background-color"),
            crate::paint::color::Color {
                r: 11,
                g: 22,
                b: 33,
                a: alpha
            }
        );
        assert_eq!(resolver.computed_style(&child), ordinary);

        parent.set_attribute("style", "color:FieldText");
        resolver.invalidate_style_cache();
        assert_eq!(
            rgb(&resolver.paint_style(&child), "background-color"),
            crate::paint::color::Color {
                r: 44,
                g: 55,
                b: 66,
                a: alpha
            }
        );
        child.set_attribute(
            "style",
            "background-color:rgba(255,0,0,0.5);forced-color-adjust:none",
        );
        resolver.invalidate_style_cache();
        assert_eq!(
            rgb(&resolver.paint_style(&child), "background-color"),
            crate::paint::color::Color {
                r: 255,
                g: 0,
                b: 0,
                a: alpha
            }
        );
        child.set_attribute("style", "background-color:rgba(255,0,0,0.5)");
        environment.set_feature("forced-colors", "none");
        resolver.set_media_environment(environment);
        assert_eq!(
            rgb(&resolver.paint_style(&child), "background-color"),
            crate::paint::color::Color {
                r: 255,
                g: 0,
                b: 0,
                a: alpha
            }
        );
    }

    #[test]
    fn visited_system_background_preserves_source_and_ordinary_cache() {
        use crate::css::{Origin, parse_stylesheet};
        let document = crate::dom::NodeHandle::document();
        let link = crate::dom::NodeHandle::element("a");
        link.set_attribute("href", "/visited");
        document.append_child(link.clone());
        let mut resolver = active_resolver();
        resolver.add_stylesheet(Origin::Author, parse_stylesheet(
            "a:link {background-color:rgba(200,100,50,0.5)} a:visited {background-color:Highlight}"
        ).unwrap());
        let ordinary = resolver.computed_style(&link);
        let alpha = rgb(&ordinary, "background-color").a;
        resolver.begin_visited_paint([link.identity()]);
        let paint = resolver.paint_style(&link);
        assert_eq!(
            rgb(&paint, "background-color"),
            crate::paint::color::Color {
                r: 11,
                g: 22,
                b: 33,
                a: alpha
            }
        );
        resolver.end_visited_paint();
        assert_eq!(resolver.computed_style(&link), ordinary);
    }

    #[test]
    fn pseudo_current_color_retains_inherited_system_identity() {
        use crate::css::{Origin, PseudoElement, parse_stylesheet};
        let document = crate::dom::NodeHandle::document();
        let node = crate::dom::NodeHandle::element("div");
        node.set_attribute("style", "color:Highlight");
        document.append_child(node.clone());
        let mut resolver = active_resolver();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("div::before {content:'x'; background:currentColor}").unwrap(),
        );
        let paint = resolver
            .paint_pseudo_style(&node, PseudoElement::Before)
            .unwrap();
        assert_eq!(
            rgb(&paint, "background-color"),
            crate::paint::color::Color::rgb(11, 22, 33)
        );
    }

    #[test]
    fn animation_overwriting_system_background_with_equal_rgb_is_forced() {
        use crate::css::{Origin, parse_stylesheet};
        let document = crate::dom::NodeHandle::document();
        let node = crate::dom::NodeHandle::element("div");
        document.append_child(node.clone());
        let mut resolver = active_resolver();
        resolver.add_stylesheet(Origin::Author, parse_stylesheet(
            "@keyframes author {from {background-color:rgb(11,22,33)}} div {background-color:Highlight; animation:author 1s paused}"
        ).unwrap());
        let ordinary = resolver.computed_style(&node);
        assert_eq!(
            rgb(&ordinary, "background-color"),
            crate::paint::color::Color::rgb(11, 22, 33)
        );
        assert!(
            ordinary
                .properties
                .system_color_name("background-color")
                .is_none()
        );
        assert_eq!(
            rgb(&resolver.paint_style(&node), "background-color"),
            crate::paint::color::Color::rgb(255, 255, 255)
        );
    }

    #[test]
    fn preserve_parent_color_uses_inheritance_not_color_equality() {
        for (declaration, inherits) in [
            ("", true),
            ("color:inherit", true),
            ("color:unset", true),
            ("color:currentColor", true),
            ("color:red", false),
            ("color:var(--explicit);--explicit:red", false),
        ] {
            let document = TreeBuilder::parse(&format!("<div style='color:red'><span id='child' style='forced-color-adjust:preserve-parent-color;background:red;{declaration}'>x</span></div>")).document();
            let mut pending = vec![document.clone()];
            let child = loop {
                let node = pending.pop().unwrap();
                if node.get_attribute("id").as_deref() == Some("child") {
                    break node;
                }
                pending.extend(node.child_nodes());
            };
            let mut resolver = StyleResolver::new();
            let mut environment = MediaEnvironment::default();
            environment.set_feature("forced-colors", "active");
            resolver.set_media_environment(environment);
            let parent = child.parent_node().unwrap();
            assert_eq!(parent.tag_name().as_deref(), Some("div"));
            let parent_style = resolver.computed_style(&parent);
            let parent_color =
                crate::paint::color::parse_color(&parent_style.get("color").unwrap().css_text())
                    .unwrap();
            assert_eq!(
                (parent_color.r, parent_color.g, parent_color.b),
                (255, 0, 0)
            );
            let ordinary = resolver.computed_style(&child);
            assert_eq!(
                ordinary.get("forced-color-adjust").unwrap().css_text(),
                "preserve-parent-color"
            );
            let computed_color =
                crate::paint::color::parse_color(&ordinary.get("color").unwrap().css_text())
                    .unwrap();
            assert_eq!(
                (computed_color.r, computed_color.g, computed_color.b),
                if inherits { (0, 0, 0) } else { (255, 0, 0) },
                "computed {declaration}"
            );
            let paint = resolver.paint_style(&child);
            let color =
                crate::paint::color::parse_color(&paint.get("color").unwrap().css_text()).unwrap();
            assert_eq!(
                (color.r, color.g, color.b),
                if inherits { (0, 0, 0) } else { (255, 0, 0) },
                "{declaration}"
            );
            assert_eq!(
                paint.get("background-color"),
                ordinary.get("background-color")
            );
            assert_eq!(
                resolver.computed_style(&child).get("color"),
                ordinary.get("color")
            );
        }
    }
    #[test]
    fn preserve_parent_color_respects_host_palette_parent_opt_out_and_reset() {
        use crate::css::{Origin, PseudoElement, parse_stylesheet};
        for (active, parent_adjust, expected) in [
            (true, "auto", (11, 22, 33)),
            (true, "none", (255, 0, 0)),
            (false, "auto", (255, 0, 0)),
        ] {
            let document = TreeBuilder::parse(&format!(
                "<div id='parent' style='color:red;forced-color-adjust:{parent_adjust}'><span id='child' style='forced-color-adjust:preserve-parent-color;color:var(--inherited);--inherited:currentColor'>x</span></div>"
            )).document();
            let mut pending = vec![document.clone()];
            let mut parent = None;
            let child = loop {
                let node = pending.pop().unwrap();
                if node.get_attribute("id").as_deref() == Some("parent") {
                    parent = Some(node.clone());
                }
                if node.get_attribute("id").as_deref() == Some("child") {
                    break node;
                }
                pending.extend(node.child_nodes());
            };
            let parent = parent.unwrap();
            let mut resolver = StyleResolver::new();
            resolver.add_stylesheet(Origin::Author, parse_stylesheet(
                "#parent::before { content:'x'; forced-color-adjust:preserve-parent-color; color:inherit }"
            ).unwrap());
            let mut environment = MediaEnvironment::default();
            environment.set_feature("forced-colors", if active { "active" } else { "none" });
            let mut palette = ForcedColorPalette::for_color_scheme(false);
            palette.canvas_text = [11, 22, 33];
            environment.forced_color_palette = Some(palette);
            resolver.set_media_environment(environment);
            let ordinary_parent = resolver.computed_style(&parent);
            for style in [
                resolver.computed_style(&child),
                resolver
                    .computed_pseudo_style(&parent, PseudoElement::Before)
                    .unwrap(),
            ] {
                let color =
                    crate::paint::color::parse_color(&style.get("color").unwrap().css_text())
                        .unwrap();
                assert_eq!(
                    (color.r, color.g, color.b),
                    expected,
                    "{active}/{parent_adjust}"
                );
            }
            assert_eq!(
                resolver.computed_style(&parent).get("color"),
                ordinary_parent.get("color")
            );
            resolver.set_media_environment(MediaEnvironment::default());
            let style = resolver.computed_style(&child);
            let color =
                crate::paint::color::parse_color(&style.get("color").unwrap().css_text()).unwrap();
            assert_eq!((color.r, color.g, color.b), (255, 0, 0));
        }
    }
    #[test]
    fn computed_forced_images_preserve_urls_and_opt_out_values() {
        for (image, url) in [
            ("linear-gradient(red,red)", false),
            ("url(a.png)", true),
            ("linear-gradient(red,red), url(\"a,b.png\")", true),
            ("u\\72l(a.png)", true),
        ] {
            for adjust in ["auto", "none", "preserve-parent-color"] {
                let document=TreeBuilder::parse(&format!("<div id='target' style='background:{image};box-shadow:0px 0px 2px red;text-shadow:0px 0px 2px red;forced-color-adjust:{adjust}'></div>")).document();
                let mut pending = vec![document.clone()];
                let node = loop {
                    let node = pending.pop().unwrap();
                    if node.get_attribute("id").as_deref() == Some("target") {
                        break node;
                    }
                    pending.extend(node.child_nodes());
                };
                let mut resolver = StyleResolver::new();
                let normal = resolver.computed_style(&node);
                assert_ne!(
                    normal.get("background-image").unwrap().css_text(),
                    "none",
                    "image parsed: {image}"
                );
                for name in ["box-shadow", "text-shadow"] {
                    assert_ne!(
                        normal.get(name).unwrap().css_text(),
                        "none",
                        "shadow parsed: {name}"
                    );
                }
                let mut environment = MediaEnvironment::default();
                environment.set_feature("forced-colors", "active");
                resolver.set_media_environment(environment);
                let computed = resolver.computed_style(&node);
                if adjust == "auto" {
                    let forced_none = ComputedValue::Keyword("none".into());
                    assert_eq!(
                        computed.get("background-image"),
                        if url {
                            normal.get("background-image")
                        } else {
                            Some(&forced_none)
                        },
                        "{image}"
                    );
                    for name in ["box-shadow", "text-shadow"] {
                        assert_eq!(computed.get(name).unwrap().css_text(), "none");
                    }
                    assert_eq!(
                        computed.get("color-scheme").unwrap().css_text(),
                        "light dark"
                    );
                    for name in ["accent-color", "scrollbar-color"] {
                        assert_eq!(computed.get(name).unwrap().css_text(), "auto");
                    }
                } else {
                    for name in ["background-image", "box-shadow", "text-shadow"] {
                        assert_eq!(computed.get(name), normal.get(name));
                    }
                }
                resolver.set_media_environment(MediaEnvironment::default());
                for name in ["background-image", "box-shadow", "text-shadow"] {
                    assert_eq!(resolver.computed_style(&node).get(name), normal.get(name));
                }
            }
        }
    }
}
