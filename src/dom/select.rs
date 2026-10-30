//! Option-list algorithms of the HTML `select` element.
//!
//! These follow the customizable-select revision of the HTML Standard: an
//! `option` belongs to its *nearest ancestor select* even when wrapped in other
//! elements, and a select's selectedness is reconciled whenever options join or
//! leave its list of options.

use super::{Node, NodeData, NodeHandle};

/// Elements that influence which `select` an `option` belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Select,
    Option,
    Optgroup,
    /// `hr` and `datalist`, which hide their descendant options.
    Barrier,
    SelectedContent,
    Other,
    NotElement,
}

fn role(node: &NodeHandle) -> Role {
    match &node.0.borrow().data {
        NodeData::Element(element) if element.is_html() => match element.local_name() {
            "select" => Role::Select,
            "option" => Role::Option,
            "optgroup" => Role::Optgroup,
            "hr" | "datalist" => Role::Barrier,
            "selectedcontent" => Role::SelectedContent,
            _ => Role::Other,
        },
        NodeData::Element(_) => Role::Other,
        _ => Role::NotElement,
    }
}

/// Returns `element`'s nearest ancestor `select`, or `None` when a `datalist`,
/// `hr`, `option` or second `optgroup` ancestor intervenes.
pub(crate) fn nearest_ancestor_select(element: &NodeHandle) -> Option<NodeHandle> {
    let mut seen_optgroup = false;
    let mut current = element.parent_node();
    while let Some(ancestor) = current {
        match role(&ancestor) {
            Role::Barrier | Role::Option => return None,
            Role::Optgroup if seen_optgroup => return None,
            Role::Optgroup => seen_optgroup = true,
            Role::Select => return Some(ancestor),
            _ => {}
        }
        current = ancestor.parent_node();
    }
    None
}

/// Returns the select's list of options in tree order.
pub(crate) fn list_of_options(select: &NodeHandle) -> Vec<NodeHandle> {
    let mut options = Vec::new();
    collect_options(select, false, &mut options);
    options
}

fn collect_options(parent: &NodeHandle, inside_optgroup: bool, options: &mut Vec<NodeHandle>) {
    let mut child = parent.first_child();
    while let Some(node) = child {
        match role(&node) {
            Role::Option => options.push(node.clone()),
            Role::Select | Role::Barrier | Role::NotElement => {}
            Role::Optgroup if inside_optgroup => {}
            Role::Optgroup => collect_options(&node, true, options),
            Role::SelectedContent | Role::Other => collect_options(&node, inside_optgroup, options),
        }
        child = node.next_sibling();
    }
}

/// Returns whether an `option` is disabled: by its own attribute, or by the
/// nearest `optgroup` ancestor reached before any `select`, `hr`, `datalist` or
/// `option` ancestor.
pub(crate) fn option_is_disabled(option: &NodeHandle) -> bool {
    if option.attribute_value_ns(None, "disabled").is_some() {
        return true;
    }
    let mut current = option.parent_node();
    while let Some(ancestor) = current {
        match role(&ancestor) {
            Role::Select | Role::Barrier | Role::Option => return false,
            Role::Optgroup => return ancestor.attribute_value_ns(None, "disabled").is_some(),
            _ => {}
        }
        current = ancestor.parent_node();
    }
    false
}

/// Returns the select's display size: a positive `size` attribute, otherwise
/// 4 for `multiple` selects and 1 for the rest.
pub(crate) fn display_size(select: &NodeHandle) -> u32 {
    let parsed = select
        .attribute_value_ns(None, "size")
        .and_then(|value| parse_non_negative_integer(&value));
    match parsed {
        Some(size) if size > 0 => size,
        _ if is_multiple(select) => 4,
        _ => 1,
    }
}

/// HTML "rules for parsing non-negative integers": leading ASCII whitespace,
/// an optional `+`, then digits; trailing content is ignored.
fn parse_non_negative_integer(value: &str) -> Option<u32> {
    let value = value.trim_start_matches(['\t', '\n', '\x0c', '\r', ' ']);
    let value = value.strip_prefix('+').unwrap_or(value);
    let digits = value.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    Some(
        value[..digits]
            .parse::<u64>()
            .map_or(u32::MAX, |size| size.min(u32::MAX as u64) as u32),
    )
}

fn is_multiple(select: &NodeHandle) -> bool {
    select.attribute_value_ns(None, "multiple").is_some()
}

/// Writes an option's selectedness without touching its dirtiness.
fn store_selectedness(option: &NodeHandle, selected: bool) {
    if let NodeData::Element(element) = &mut option.0.borrow_mut().data {
        element.selected = selected;
    }
}

/// Runs the selectedness setting algorithm. Returns whether any option's
/// selectedness changed.
pub(crate) fn run_selectedness_setting(select: &NodeHandle) -> bool {
    if is_multiple(select) {
        return false;
    }
    let mut changed = false;
    let mut first_enabled = None;
    let mut last_selected: Option<NodeHandle> = None;
    for option in list_of_options(select) {
        if option.selected() {
            if let Some(previous) = last_selected.replace(option.clone()) {
                store_selectedness(&previous, false);
                changed = true;
            }
        }
        if first_enabled.is_none() && !option_is_disabled(&option) {
            first_enabled = Some(option);
        }
    }
    if last_selected.is_none()
        && let Some(option) = first_enabled
        && display_size(select) == 1
    {
        store_selectedness(&option, true);
        changed = true;
    }
    changed
}

/// Runs the selectedness setting algorithm for every `select` under `root`,
/// including declarative shadow trees. The parser calls this once its tree is
/// complete, which yields the same selectedness as per-insertion resets.
pub(crate) fn reset_selects_in(root: &NodeHandle) {
    let mut child = root.first_child();
    while let Some(node) = child {
        if role(&node) != Role::NotElement {
            if role(&node) == Role::Select {
                run_selectedness_setting(&node);
            }
            if let Some(shadow) = node.shadow_root() {
                reset_selects_in(&shadow);
            }
            reset_selects_in(&node);
        }
        child = node.next_sibling();
    }
}

/// Sets an option's selectedness. Selecting an option of a single-selection
/// select deselects the other options in that select's list of options.
pub(crate) fn set_option_selectedness(option: &NodeHandle, selected: bool, mark_dirty: bool) {
    if let NodeData::Element(element) = &mut option.0.borrow_mut().data {
        element.selected = selected;
        element.dirty_selectedness |= mark_dirty;
    }
    if !selected {
        return;
    }
    let Some(select) = nearest_ancestor_select(option).filter(|select| !is_multiple(select)) else {
        return;
    };
    for other in list_of_options(&select) {
        if other != *option {
            store_selectedness(&other, false);
        }
    }
}

/// The `selectedIndex` and `value` setters: deselects every option and marks
/// the chosen one selected and dirty, without the selectedness setting
/// algorithm. Returns whether any option's selectedness changed.
pub(crate) fn select_only(select: &NodeHandle, chosen: Option<&NodeHandle>) -> bool {
    let mut changed = false;
    for option in list_of_options(select) {
        let selected = chosen == Some(&option);
        if option.selected() != selected {
            changed = true;
        }
        if let NodeData::Element(element) = &mut option.0.borrow_mut().data {
            element.selected = selected;
            element.dirty_selectedness |= selected;
        }
    }
    changed
}

/// Applies a change of a non-dirty option's `selected` content attribute: the
/// selectedness follows the attribute and the option's select is reset.
/// Returns the option's select when its `selectedcontent` needs updating.
pub(crate) fn apply_selected_attribute(option: &NodeHandle) -> Option<NodeHandle> {
    let (dirty, before) = match &option.0.borrow().data {
        NodeData::Element(element) => (element.dirty_selectedness, element.selected),
        _ => return None,
    };
    if dirty {
        return None;
    }
    let selected = option.attribute_value_ns(None, "selected").is_some();
    set_option_selectedness(option, selected, false);
    let select = nearest_ancestor_select(option)?;
    let changed = run_selectedness_setting(&select);
    (changed || before != option.selected() || selected).then_some(select)
}

/// One `select` whose descendant `selectedcontent` elements need updating.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SelectedContentUpdate {
    pub(crate) select: NodeHandle,
    /// Removal and move steps defer the update to a microtask; insertion and
    /// selection changes update synchronously.
    pub(crate) deferred: bool,
}

/// The selects related to the options and `selectedcontent` elements of a
/// subtree, captured before a tree mutation.
#[derive(Default)]
pub(crate) struct SubtreeSelects {
    /// Each option with its nearest ancestor select at capture time.
    options: Vec<(NodeHandle, Option<NodeHandle>)>,
    /// Each `selectedcontent` with the select it reflected at capture time.
    selected_contents: Vec<(NodeHandle, Option<NodeHandle>)>,
}

impl SubtreeSelects {
    /// Captures the options and `selectedcontent` elements in `root`'s
    /// inclusive subtree, with each option's current nearest ancestor select.
    pub(crate) fn capture(root: &NodeHandle) -> Self {
        let mut captured = Self::default();
        captured.visit(root);
        captured
    }

    fn visit(&mut self, node: &NodeHandle) {
        match role(node) {
            Role::NotElement => return,
            Role::Option => self
                .options
                .push((node.clone(), nearest_ancestor_select(node))),
            Role::SelectedContent => self
                .selected_contents
                .push((node.clone(), selected_content_select(node))),
            _ => {}
        }
        let mut child = node.first_child();
        while let Some(current) = child {
            self.visit(&current);
            child = current.next_sibling();
        }
    }

    /// Applies the option and `selectedcontent` insertion, removing or moving
    /// steps after the mutation, and returns the selects whose
    /// `selectedcontent` elements need updating.
    pub(crate) fn finish(self, mutation: TreeMutation) -> MutationOutcome {
        let deferred = mutation != TreeMutation::Insert;
        let mut reset = Vec::new();
        let mut updates: Vec<SelectedContentUpdate> = Vec::new();
        let mut push = |select: NodeHandle, deferred: bool| {
            if !updates.iter().any(|update| update.select == select) {
                updates.push(SelectedContentUpdate { select, deferred });
            }
        };
        for (option, old_select) in self.options {
            let new_select = nearest_ancestor_select(&option);
            if old_select == new_select {
                continue;
            }
            // Leaving a select runs the removing steps: a microtask update.
            if let Some(select) = old_select {
                reset.push(select.clone());
                if run_selectedness_setting(&select) || option.selected() {
                    push(select, true);
                }
            }
            if let Some(select) = new_select {
                reset.push(select.clone());
                let changed = run_selectedness_setting(&select);
                // Insertion's post-connection steps update synchronously when
                // the inserted option is selected or the reset changed the
                // selection.
                if option.selected() || changed {
                    push(select, deferred);
                }
            }
        }
        // Inserted `selectedcontent` elements update in their post-connection
        // steps; moved ones only when they now reflect a different select.
        if mutation != TreeMutation::Remove {
            for (selected_content, old_select) in self.selected_contents {
                let new_select = selected_content_select(&selected_content);
                if mutation == TreeMutation::Move && new_select == old_select {
                    continue;
                }
                if let Some(select) = new_select {
                    push(select, deferred);
                }
            }
        }
        reset.dedup();
        MutationOutcome { reset, updates }
    }
}

/// The result of applying the select steps of one tree mutation.
pub(crate) struct MutationOutcome {
    /// Selects whose options' selectedness may have changed.
    pub(crate) reset: Vec<NodeHandle>,
    /// Selects whose `selectedcontent` elements need updating.
    pub(crate) updates: Vec<SelectedContentUpdate>,
}

/// The kind of tree mutation whose `select` steps [`SubtreeSelects`] applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TreeMutation {
    Insert,
    Remove,
    /// A state-preserving `moveBefore()`.
    Move,
}

/// Returns the select a `selectedcontent` element reflects, or `None` when it
/// is disabled by an `option`, `selectedcontent` or second `select` ancestor.
pub(crate) fn selected_content_select(selected_content: &NodeHandle) -> Option<NodeHandle> {
    let mut nearest = None;
    let mut current = selected_content.parent_node();
    while let Some(ancestor) = current {
        match role(&ancestor) {
            Role::Select if nearest.is_some() => return None,
            Role::Select => nearest = Some(ancestor.clone()),
            Role::Option | Role::SelectedContent => return None,
            _ => {}
        }
        current = ancestor.parent_node();
    }
    nearest.filter(|select| !is_multiple(select))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_belong_to_their_nearest_ancestor_select() {
        let element = |name: &str, id: &str| {
            let node = NodeHandle::element(name);
            if !id.is_empty() {
                node.set_attribute("id", id);
            }
            node
        };
        let select = element("select", "s");
        let wrapped = element("option", "a");
        let div = element("div", "");
        div.append_child(wrapped.clone());
        select.append_child(div);
        let outer = element("optgroup", "");
        let grouped = element("option", "b");
        let group_div = element("div", "");
        group_div.append_child(grouped.clone());
        outer.append_child(group_div);
        let inner = element("optgroup", "");
        let nested = element("option", "c");
        inner.append_child(nested.clone());
        outer.append_child(inner);
        select.append_child(outer);
        let hidden = element("option", "d");
        let datalist = element("datalist", "");
        datalist.append_child(hidden.clone());
        select.append_child(datalist);

        assert_eq!(
            list_of_options(&select),
            vec![wrapped.clone(), grouped.clone()]
        );
        assert_eq!(nearest_ancestor_select(&wrapped), Some(select.clone()));
        assert_eq!(nearest_ancestor_select(&grouped), Some(select.clone()));
        assert_eq!(nearest_ancestor_select(&nested), None);
        assert_eq!(nearest_ancestor_select(&hidden), None);
    }

    #[test]
    fn selectedness_setting_selects_the_first_enabled_option_once() {
        let select = NodeHandle::element("select");
        let first = NodeHandle::element("option");
        first.set_attribute("disabled", "");
        let second = NodeHandle::element("option");
        select.append_child(first.clone());
        select.append_child(second.clone());
        assert!(run_selectedness_setting(&select));
        assert!(!first.selected() && second.selected());
        assert!(!run_selectedness_setting(&select));
        select.set_attribute("size", "2");
        store_selectedness(&second, false);
        assert!(
            !run_selectedness_setting(&select),
            "list boxes keep no selection"
        );
    }

    #[test]
    fn display_size_parses_non_negative_integers() {
        assert_eq!(parse_non_negative_integer(" +3px"), Some(3));
        assert_eq!(parse_non_negative_integer("-1"), None);
        assert_eq!(parse_non_negative_integer("x"), None);
    }
}
