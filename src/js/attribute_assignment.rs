//! Opt-in diagnostics for exact ordinary style/class attribute assignments.
use super::*;

fn previous_value(node: &NodeHandle, name: &str, value: &JsString) -> (Option<usize>, bool) {
    let old = node.get_attribute_utf16(name);
    (
        old.as_ref().map(Vec::len),
        old.as_ref()
            .is_some_and(|units| units.iter().copied().eq(value.iter())),
    )
}

#[derive(Debug, serde::Serialize)]
struct StyleDifference {
    changed_properties: Vec<String>,
    order_changed: bool,
}

fn style_difference(old: &str, new: &str) -> StyleDifference {
    let declarations = |source: &str| {
        let declarations = crate::css::parse_style_attribute(source);
        let order: Vec<_> = declarations.iter().map(|d| d.name.clone()).collect();
        let mut values: BTreeMap<String, (crate::css::Value, bool)> = BTreeMap::new();
        for declaration in declarations {
            if values
                .get(&declaration.name)
                .is_some_and(|(_, important)| *important && !declaration.important)
            {
                continue;
            }
            values.insert(declaration.name, (declaration.value, declaration.important));
        }
        (order, values)
    };
    let (old_order, old_values) = declarations(old);
    let (new_order, new_values) = declarations(new);
    let names: std::collections::BTreeSet<_> = old_values.keys().chain(new_values.keys()).collect();
    StyleDifference {
        changed_properties: names
            .into_iter()
            .filter(|name| old_values.get(*name) != new_values.get(*name))
            .map(Clone::clone)
            .collect(),
        order_changed: old_order != new_order,
    }
}

fn parsed_difference(node: &NodeHandle, name: &str, value: &JsString) -> Option<StyleDifference> {
    if !name.eq_ignore_ascii_case("style") {
        return None;
    }
    let old = String::from_utf16(&node.get_attribute_utf16(name).unwrap_or_default()).ok()?;
    let new = value.to_std_string().ok()?;
    Some(style_difference(&old, &new))
}

/// Emits equality and changed declaration names, never property values.
pub(super) fn trace(node: &NodeHandle, name: &str, value: &JsString) {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if !*ENABLED.get_or_init(|| std::env::var_os("OMOIKANE_TRACE_ATTRIBUTE_SET").is_some()) {
        return;
    }
    if !name.eq_ignore_ascii_case("style") && !name.eq_ignore_ascii_case("class") {
        return;
    }
    let (old_length, same_value) = previous_value(node, name, value);
    eprintln!(
        "OMOIKANE_ATTRIBUTE_SET {}",
        serde_json::json!({
            "node": node.identity(), "name": name,
            "old_length": old_length, "new_length": value.len(),
            "same_value": same_value,
            "declarations": parsed_difference(node, name, value),
        })
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declaration_diagnostics_separate_order_value_and_priority_changes() {
        let difference = style_difference("width:1px;height:2px", "height:2px;width:1px");
        assert!(difference.changed_properties.is_empty());
        assert!(difference.order_changed);
        let difference = style_difference("width:1px;height:2px", "width:2px;height:2px");
        assert_eq!(difference.changed_properties, ["width"]);
        assert!(!difference.order_changed);
        let difference = style_difference("color:red!important", "color:red");
        assert_eq!(difference.changed_properties, ["color"]);
        let difference = style_difference("color:red!important;color:blue", "color:red!important");
        assert!(difference.changed_properties.is_empty());
    }

    #[test]
    fn equality_distinguishes_missing_empty_and_exact_utf16_without_mutating_the_node() {
        let node = NodeHandle::element("div");
        let empty = JsString::from("");
        assert_eq!(previous_value(&node, "style", &empty), (None, false));
        node.set_attribute("style", "");
        assert_eq!(previous_value(&node, "style", &empty), (Some(0), true));
        node.set_attribute_utf16("style", &[0xd800]);
        let same = JsString::from(&[0xd800u16][..]);
        let different = JsString::from(&[0xd801u16][..]);
        assert_eq!(previous_value(&node, "style", &same), (Some(1), true));
        assert_eq!(previous_value(&node, "style", &different), (Some(1), false));
        assert_eq!(node.get_attribute_utf16("style"), Some(vec![0xd800]));
    }
}
