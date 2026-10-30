//! HTML rendering rules for `select` elements with base appearance
//! (customizable select, Issue #1573).
//!
//! The rules are kept as UA-origin declarations so author rules, `revert`
//! and CSSOM see them exactly like the HTML UA stylesheet. Which part an
//! element plays is decided from the DOM and the owning select's computed
//! `appearance`; the declarations themselves are fixed.

use super::{Candidate, push_builtin_ua_candidate};
use crate::css::Specificity;
use crate::css::matcher::PseudoElement;
use crate::dom::select::{self, is_drop_down, select_button};
use crate::dom::{Node, NodeHandle};

/// The role an element or pseudo-element has inside a base appearance select.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BaseSelectPart {
    /// A base appearance drop-down `select`.
    DropDown { disabled: bool },
    /// A base appearance list box `select` (`multiple` or `size` > 1).
    ListBox,
    /// `::picker(select)` with base appearance.
    Picker,
    /// `select::picker-icon`.
    PickerIcon,
    /// An `option` of a base appearance select.
    Option { in_optgroup: bool },
    /// `option::checkmark`; hidden unless the option is selected.
    Checkmark { checked: bool },
    /// An `optgroup` of a base appearance select.
    OptGroup,
    /// The `legend` labelling such an `optgroup`.
    Legend,
    /// The select's leading `button`, which takes the place of its label.
    Button,
}

/// The base appearance part of a `select` whose own computed `appearance`
/// is `base-select`.
pub(super) fn select_part(node: &NodeHandle) -> BaseSelectPart {
    if is_drop_down(node) {
        BaseSelectPart::DropDown {
            disabled: node.has_attribute("disabled"),
        }
    } else {
        BaseSelectPart::ListBox
    }
}

/// Returns `false` for the customizable select pseudo-elements on elements
/// that cannot have them: only a drop-down `select` has `::picker(select)` and
/// `::picker-icon`, and only an `option` has `::checkmark`.
pub(super) fn pseudo_element_applies(node: &NodeHandle, pseudo: PseudoElement) -> bool {
    match pseudo {
        PseudoElement::Picker | PseudoElement::PickerIcon => {
            node.is_html_element() && node.has_tag_name("select") && is_drop_down(node)
        }
        PseudoElement::Checkmark => node.is_html_element() && node.has_tag_name("option"),
        _ => true,
    }
}

/// The select whose base appearance decides `node`'s part, together with
/// the part `node` plays when that select uses base appearance.
pub(super) fn owner_and_part(
    node: &NodeHandle,
    pseudo: Option<PseudoElement>,
) -> Option<(NodeHandle, BaseSelectPart)> {
    if !node.is_html_element() {
        return None;
    }
    match pseudo {
        Some(PseudoElement::PickerIcon) if node.has_tag_name("select") && is_drop_down(node) => {
            Some((node.clone(), BaseSelectPart::PickerIcon))
        }
        Some(PseudoElement::Checkmark) if node.has_tag_name("option") => {
            let owner = select::nearest_ancestor_select(node)?;
            Some((
                owner,
                BaseSelectPart::Checkmark {
                    checked: node.selected(),
                },
            ))
        }
        Some(_) => None,
        None => element_owner_and_part(node),
    }
}

fn element_owner_and_part(node: &NodeHandle) -> Option<(NodeHandle, BaseSelectPart)> {
    if node.has_tag_name("option") {
        let owner = select::nearest_ancestor_select(node)?;
        let in_optgroup = ancestors(node)
            .take_while(|ancestor| !ancestor.has_tag_name("select"))
            .any(|ancestor| ancestor.has_tag_name("optgroup"));
        return Some((owner, BaseSelectPart::Option { in_optgroup }));
    }
    if node.has_tag_name("optgroup") {
        return Some((ancestor_select(node)?, BaseSelectPart::OptGroup));
    }
    if node.has_tag_name("legend") {
        let optgroup = node
            .parent_node()
            .filter(|parent| parent.has_tag_name("optgroup"))?;
        return Some((ancestor_select(&optgroup)?, BaseSelectPart::Legend));
    }
    if node.has_tag_name("button") {
        let parent = node.parent_node().filter(|parent| {
            parent.is_html_element() && parent.has_tag_name("select") && is_drop_down(parent)
        })?;
        return (select_button(&parent)?.identity() == node.identity())
            .then_some((parent, BaseSelectPart::Button));
    }
    None
}

fn ancestors(node: &NodeHandle) -> impl Iterator<Item = NodeHandle> {
    std::iter::successors(node.parent_node(), |ancestor| ancestor.parent_node())
}

fn ancestor_select(node: &NodeHandle) -> Option<NodeHandle> {
    ancestors(node).find(|ancestor| ancestor.is_html_element() && ancestor.has_tag_name("select"))
}

/// The HTML UA declarations for `part`, as CSS declaration-block text.
fn declarations(part: BaseSelectPart) -> &'static str {
    match part {
        BaseSelectPart::DropDown { disabled: false } => concat!(
            "background-color: transparent; border: 1px solid currentColor; user-select: none;",
            "box-sizing: border-box; border-radius: 0.5em; padding-block: 0.25em;",
            "padding-inline: 0.5em; display: inline-flex; gap: 0.5em; cursor: default;",
            "text-transform: initial; text-indent: initial; text-shadow: initial;",
            "text-align: initial"
        ),
        BaseSelectPart::DropDown { disabled: true } => concat!(
            "background-color: transparent; border: 1px solid currentColor; user-select: none;",
            "box-sizing: border-box; border-radius: 0.5em; padding-block: 0.25em;",
            "padding-inline: 0.5em; display: inline-flex; gap: 0.5em; cursor: not-allowed;",
            "text-transform: initial; text-indent: initial; text-shadow: initial;",
            "text-align: initial"
        ),
        BaseSelectPart::ListBox => concat!(
            "background-color: transparent; border: 1px solid currentColor; user-select: none;",
            "box-sizing: border-box; overflow: auto; display: inline-block"
        ),
        BaseSelectPart::Picker => concat!(
            "box-sizing: border-box; border: 1px solid; padding: 0; color: CanvasText;",
            "background-color: Canvas; margin: 0; inset: auto; overflow: auto"
        ),
        BaseSelectPart::PickerIcon => concat!(
            "content: counter(fake-counter-name, disclosure-open); display: block;",
            "margin-inline-start: auto"
        ),
        // `bolder` and `normal` over the initial weight; computed weights are
        // serialized as numbers.
        BaseSelectPart::Option { in_optgroup: false } => concat!(
            "min-inline-size: 24px; min-block-size: 24px; padding-inline: 0.5em;",
            "padding-block-start: 0; padding-block-end: 0; display: flex;",
            "align-items: center; gap: 0.5em; white-space: nowrap"
        ),
        BaseSelectPart::Option { in_optgroup: true } => concat!(
            "min-inline-size: 24px; min-block-size: 24px; padding-inline: 0.5em;",
            "padding-block-start: 0; padding-block-end: 0; display: flex;",
            "align-items: center; gap: 0.5em; white-space: nowrap; font-weight: 400"
        ),
        BaseSelectPart::Checkmark { checked: true } => "content: '\\2713' / ''",
        BaseSelectPart::Checkmark { checked: false } => {
            "content: '\\2713' / ''; visibility: hidden"
        }
        BaseSelectPart::OptGroup => "display: block; font-weight: 700",
        BaseSelectPart::Legend => concat!(
            "padding-inline: 0.5em; min-block-size: 1lh; display: block;",
            "unicode-bidi: isolate; font-weight: 700"
        ),
        BaseSelectPart::Button => "display: contents",
    }
}

/// Pushes `part`'s UA declarations as UA-origin cascade candidates.
pub(super) fn push_candidates(
    part: BaseSelectPart,
    source_order: &mut usize,
    candidates: &mut Vec<Candidate>,
) {
    let rule_order = *source_order;
    for declaration in crate::css::parse_style_attribute(declarations(part)) {
        push_builtin_ua_candidate(
            super::canonical_property_name(&declaration.name).to_string(),
            declaration.value,
            Specificity {
                ids: 0,
                classes: 1,
                elements: 1,
            },
            rule_order,
            source_order,
            candidates,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_part_declares_parseable_rules() {
        for part in [
            BaseSelectPart::DropDown { disabled: false },
            BaseSelectPart::DropDown { disabled: true },
            BaseSelectPart::ListBox,
            BaseSelectPart::Picker,
            BaseSelectPart::PickerIcon,
            BaseSelectPart::Option { in_optgroup: false },
            BaseSelectPart::Option { in_optgroup: true },
            BaseSelectPart::Checkmark { checked: true },
            BaseSelectPart::Checkmark { checked: false },
            BaseSelectPart::OptGroup,
            BaseSelectPart::Legend,
            BaseSelectPart::Button,
        ] {
            for declaration in declarations(part).split(';') {
                assert!(
                    !crate::css::parse_style_attribute(declaration).is_empty(),
                    "{part:?} drops `{declaration}`"
                );
            }
        }
    }
}
