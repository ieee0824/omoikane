//! Default form controls, indexed only for an unchanged selector match pass.

use super::*;
use crate::dom::form_controls::{html_local_name, is_submit_button};

/// Matches content defaults and the first submit button of each form owner.
pub(super) fn matches(node: &NodeHandle, cache: &mut SelectorMatchCache) -> bool {
    match html_local_name(node).as_deref() {
        Some("option") => return node.attribute_value_ns(None, "selected").is_some(),
        Some("input") => {
            let kind = node.attribute_value_ns(None, "type").unwrap_or_default();
            if kind.eq_ignore_ascii_case("checkbox") || kind.eq_ignore_ascii_case("radio") {
                return node.attribute_value_ns(None, "checked").is_some();
            }
        }
        Some("button") => {}
        _ => return false,
    }
    if !is_submit_button(node) {
        return false;
    }
    let root = tree_root(node);
    use std::collections::hash_map::Entry;
    let defaults = match cache.default_submit_buttons.entry(root.identity()) {
        Entry::Occupied(entry) => entry.into_mut(),
        Entry::Vacant(entry) => {
            #[cfg(test)]
            {
                cache.default_submit_builds += 1;
            }
            entry.insert(default_submit_buttons(&root))
        }
    };
    defaults.contains(&node.identity())
}

fn tree_root(node: &NodeHandle) -> NodeHandle {
    let mut root = node.clone();
    while let Some(parent) = root.parent_node() {
        root = parent;
    }
    root
}

fn connected(root: &NodeHandle) -> bool {
    let mut root = root.clone();
    while let Some(host) = root.shadow_host() {
        root = tree_root(&host);
    }
    root.node_type() == NodeType::Document
}

fn default_submit_buttons(root: &NodeHandle) -> HashSet<usize> {
    // Only IDs survive this function. Temporary control handles never become
    // part of the matcher cache, so it cannot retain removed DOM subtrees.
    let mut first_ids = HashMap::<Vec<u16>, Option<usize>>::new();
    let mut controls = Vec::new();
    let mut pending = vec![(root.clone(), None)];
    while let Some((node, ancestor_form)) = pending.pop() {
        let tag = html_local_name(&node);
        let form = (tag.as_deref() == Some("form")).then(|| node.identity());
        if let Some(id) = node.attribute_value_ns_utf16(None, "id") {
            first_ids.entry(id).or_insert(form);
        }
        if matches!(tag.as_deref(), Some("input" | "button")) && is_submit_button(&node) {
            controls.push((node.clone(), ancestor_form));
        }
        let next_form = form.or(ancestor_form);
        for child in node.child_nodes().into_iter().rev() {
            pending.push((child, next_form));
        }
    }
    let connected = connected(root);
    let mut seen_forms = HashSet::new();
    let mut defaults = HashSet::new();
    for (control, ancestor_form) in controls {
        let parser_owner = control
            .peek_parser_form_owner()
            .filter(|owner| html_local_name(owner).as_deref() == Some("form"));
        let owner = if let Some(owner) = parser_owner {
            Some(owner.identity())
        } else if connected && let Some(id) = control.attribute_value_ns_utf16(None, "form") {
            if id.is_empty() {
                None
            } else {
                first_ids.get(&id).copied().flatten()
            }
        } else {
            ancestor_form
        };
        if let Some(owner) = owner
            && seen_forms.insert(owner)
        {
            defaults.insert(control.identity());
        }
    }
    defaults
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_index_is_shared_by_candidates_in_the_same_root() {
        let doc = crate::html::TreeBuilder::parse(
            "<form><button id=a>A</button><button id=b>B</button></form>",
        )
        .document();
        let a = doc.query_selector("#a").unwrap();
        let b = doc.query_selector("#b").unwrap();
        let mut cache = SelectorMatchCache::default();
        assert!(matches(&a, &mut cache));
        assert!(!matches(&b, &mut cache));
        assert_eq!(cache.default_submit_buttons.len(), 1);
        assert_eq!(cache.default_submit_builds, 1);
    }

    #[test]
    fn cached_default_controls_do_not_retain_the_tree() {
        let mut cache = SelectorMatchCache::default();
        let weak = {
            let doc = crate::html::TreeBuilder::parse("<form><button>A</button></form>").document();
            let button = doc.query_selector("button").unwrap();
            assert!(matches(&button, &mut cache));
            doc.downgrade()
        };
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn matching_inside_a_read_view_does_not_update_parser_owner_metadata() {
        let doc = crate::html::TreeBuilder::parse("<form id=f></form><input id=i type=submit>")
            .document();
        let form = doc.query_selector("#f").unwrap();
        let input = doc.query_selector("#i").unwrap();
        input.set_parser_form_owner(&form);
        assert_eq!(input.parser_form_owner(), Some(form.clone()));
        let parent = form.parent_node().unwrap();
        parent.remove_child(&form);
        parent.append_child(form);
        let mut cache = SelectorMatchCache::default();
        assert!(input.with_tag_name(|_| !matches(&input, &mut cache)));
    }
}
