//! Typed inheritance retains values, custom-property order and color source identity.
use super::*;

#[test]
fn inherited_lookup_copy_by_id_matches_name_copy_with_color_metadata() {
    for &id in PropertyId::ALL {
        let mut source = PropertyMap::new();
        source.insert(id, ComputedValue::Keyword("source".into()));
        source.mark_system_color(id.as_str(), "CanvasText");
        let mut original = PropertyMap::new();
        original.insert(id, ComputedValue::Number(9.0));
        original.mark_system_color(id.as_str(), "AccentColor");
        let empty = PropertyMap::new();
        for source in [&source, &empty] {
            let mut expected = original.clone();
            expected.insert_from(id.as_str(), source);
            let mut actual = original.clone();
            actual.insert_from_by_id(id, source);
            assert_eq!(actual, expected, "{}", id.as_str());
        }
    }
}

#[test]
fn inherited_lookup_custom_iteration_matches_the_full_map_filter() {
    let mut values = PropertyMap::new();
    for name in [
        "color",
        "width",
        "--Z",
        "--a",
        "--font-family",
        "vendor-field",
        "transition",
    ] {
        values.insert(name, ComputedValue::Keyword(name.into()));
    }
    let expected: Vec<_> = values
        .iter()
        .filter(|(name, _)| name.starts_with("--"))
        .collect();
    let actual: Vec<_> = values.custom_properties().collect();
    assert_eq!(actual, expected);
    assert_eq!(actual.len(), 3);
}

#[test]
fn inherited_lookup_preserves_element_and_pseudo_inheritance() {
    let parent = NodeHandle::element("div");
    let child = NodeHandle::element("span");
    parent.set_attribute(
        "style",
        "color:CanvasText;font-size:20px;direction:rtl;margin:8px;--tone:teal",
    );
    parent.append_child(child.clone());
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        super::super::parse_stylesheet("span::before{content:'hello'}").unwrap(),
    );
    let before = resolver.computed_style(&parent);
    let normal = resolver.computed_style(&child);
    let pseudo = resolver
        .computed_pseudo_style(&child, PseudoElement::Before)
        .unwrap();
    for style in [&normal, &pseudo] {
        for name in ["color", "font-size", "direction", "--tone"] {
            assert_eq!(style.get(name), before.get(name), "{name}");
        }
        // The existing computed map omits this non-inherited property.
        assert_eq!(style.get("margin-left"), None);
        assert_eq!(
            style.properties.system_color_name("color"),
            Some("canvastext")
        );
    }
    assert_eq!(before.get("margin-left"), Some(&ComputedValue::Px(8.0)));
    assert_eq!(before, resolver.computed_style(&parent));
}
