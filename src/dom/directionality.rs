//! HTML element directionality used by the Selectors `:dir()` pseudo-class.
use super::{Node, NodeHandle, NodeType, input_value};
use unicode_bidi::{BidiClass, bidi_class};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    Ltr,
    Rtl,
}

impl NodeHandle {
    /// Returns whether this element has the requested HTML directionality.
    pub(crate) fn matches_directionality(&self, requested: &str) -> bool {
        let requested = match requested.trim().to_ascii_lowercase().as_str() {
            "ltr" => Direction::Ltr,
            "rtl" => Direction::Rtl,
            _ => return false,
        };
        self.node_type() == NodeType::Element && directionality(self) == requested
    }

    /// Returns the low-priority CSS `direction` hint contributed by HTML.
    ///
    /// Elements with no valid `dir` state inherit the CSS property through the
    /// flattened tree even though their HTML directionality uses the ordinary
    /// DOM parent. `<bdi>` is the one implicit-auto exception.
    pub(crate) fn directionality_presentational_hint(&self) -> Option<&'static str> {
        if self.node_type() != NodeType::Element
            || (dir_state(self).is_none() && !is_html_named(self, "bdi"))
        {
            return None;
        }
        Some(if directionality(self) == Direction::Rtl {
            "rtl"
        } else {
            "ltr"
        })
    }
}

fn directionality(element: &NodeHandle) -> Direction {
    match dir_state(element) {
        Some("ltr") => Direction::Ltr,
        Some("rtl") => Direction::Rtl,
        Some("auto") => auto_directionality(element).unwrap_or(Direction::Ltr),
        _ if is_html_named(element, "bdi") => {
            auto_directionality(element).unwrap_or(Direction::Ltr)
        }
        _ if is_html_named(element, "input") && input_type(element) == "tel" => Direction::Ltr,
        _ => parent_directionality(element),
    }
}

fn parent_directionality(element: &NodeHandle) -> Direction {
    let Some(parent) = element.parent_node() else {
        return Direction::Ltr;
    };
    if parent.node_type() == NodeType::DocumentFragment {
        return parent
            .shadow_host()
            .map_or(Direction::Ltr, |host| directionality(&host));
    }
    if parent.node_type() == NodeType::Element {
        directionality(&parent)
    } else {
        Direction::Ltr
    }
}

fn auto_directionality(element: &NodeHandle) -> Option<Direction> {
    if is_auto_form_control(element) {
        return strong_direction(&form_control_value(element));
    }
    if is_html_named(element, "slot") && element.containing_shadow_root().is_some() {
        let assigned = element.assigned_nodes(false);
        if !assigned.is_empty() {
            for child in assigned {
                let result = match child.node_type() {
                    NodeType::Text => child.data().as_deref().and_then(strong_direction),
                    NodeType::Element => contained_text_directionality(&child, true),
                    _ => None,
                };
                if result.is_some() {
                    return result;
                }
            }
            return None;
        }
    }
    contained_text_directionality(element, false)
}

fn contained_text_directionality(element: &NodeHandle, exclude_root: bool) -> Option<Direction> {
    if exclude_root && excludes_auto_text(element) {
        return None;
    }
    for child in element.child_nodes() {
        if child.node_type() == NodeType::Element {
            if excludes_auto_text(&child) {
                continue;
            }
            if is_html_named(&child, "slot") && child.containing_shadow_root().is_some() {
                return child
                    .containing_shadow_root()
                    .and_then(|root| root.shadow_host())
                    .map(|host| directionality(&host));
            }
            if let Some(result) = contained_text_directionality(&child, false) {
                return Some(result);
            }
        } else if child.node_type() == NodeType::Text
            && let Some(result) = child.data().as_deref().and_then(strong_direction)
        {
            return Some(result);
        }
    }
    None
}

fn excludes_auto_text(element: &NodeHandle) -> bool {
    is_html_named(element, "bdi")
        || is_html_named(element, "script")
        || is_html_named(element, "style")
        || is_html_named(element, "textarea")
        || dir_state(element).is_some()
}

fn strong_direction(value: &str) -> Option<Direction> {
    value
        .chars()
        .find_map(|character| match bidi_class(character) {
            BidiClass::L => Some(Direction::Ltr),
            BidiClass::R | BidiClass::AL => Some(Direction::Rtl),
            _ => None,
        })
}

fn form_control_value(element: &NodeHandle) -> String {
    if let Some(value) = element.directionality_control_value() {
        return value;
    }
    if is_html_named(element, "input") {
        return element
            .attribute_value_ns(None, "value")
            .unwrap_or_default();
    }
    let mut value = element
        .child_nodes()
        .into_iter()
        .filter(|child| child.node_type() == NodeType::Text)
        .filter_map(|child| child.data())
        .collect::<String>()
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    if value.starts_with('\n') {
        value.remove(0);
    }
    value
}

fn is_auto_form_control(element: &NodeHandle) -> bool {
    if is_html_named(element, "textarea") {
        return true;
    }
    is_html_named(element, "input")
        && matches!(
            input_type(element),
            "hidden"
                | "text"
                | "search"
                | "tel"
                | "url"
                | "email"
                | "password"
                | "submit"
                | "reset"
                | "button"
        )
}

fn input_type(element: &NodeHandle) -> &'static str {
    input_value::canonical_type(
        &element
            .attribute_value_ns(None, "type")
            .unwrap_or_else(|| "text".to_owned()),
    )
}

fn dir_state(element: &NodeHandle) -> Option<&'static str> {
    if !element.is_html_element() {
        return None;
    }
    let value = element.attribute_value_ns(None, "dir")?;
    if value.eq_ignore_ascii_case("ltr") {
        Some("ltr")
    } else if value.eq_ignore_ascii_case("rtl") {
        Some("rtl")
    } else if value.eq_ignore_ascii_case("auto") {
        Some("auto")
    } else {
        None
    }
}

fn is_html_named(element: &NodeHandle, name: &str) -> bool {
    element.is_html_element()
        && element
            .with_local_name(|local| local.is_some_and(|local| local.eq_ignore_ascii_case(name)))
}
