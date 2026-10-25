//! Owned form-control indices shared for one unchanged selector match pass.
use super::*;
use crate::dom::form_controls::{html_local_name, is_submit_button};

#[derive(Debug, Default)]
pub(super) struct FormStateIndex {
    pub(super) defaults: HashSet<usize>,
    pub(super) indeterminate_radios: HashSet<usize>,
}

#[derive(Hash, PartialEq, Eq)]
enum RadioGroup {
    Named(Option<usize>, Vec<u16>),
    Unnamed(usize),
}

enum Kind {
    Submit,
    Radio { name: Vec<u16>, checked: bool },
}

struct Control {
    node: NodeHandle,
    ancestor_form: Option<usize>,
    kind: Kind,
}

/// Borrows only the match-pass cache; no DOM handle survives index construction.
pub(super) fn for_node<'cache>(
    node: &NodeHandle,
    cache: &'cache mut SelectorMatchCache,
) -> &'cache FormStateIndex {
    use std::collections::hash_map::Entry;
    let root = tree_root(node);
    match cache.form_states.entry(root.identity()) {
        Entry::Occupied(entry) => entry.into_mut(),
        Entry::Vacant(entry) => {
            #[cfg(test)]
            {
                cache.form_state_builds += 1;
            }
            entry.insert(build(&root))
        }
    }
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

fn collect(root: &NodeHandle) -> (HashMap<Vec<u16>, Option<usize>>, Vec<Control>) {
    let mut first_ids = HashMap::new();
    let mut controls = Vec::new();
    let mut pending = vec![(root.clone(), None)];
    while let Some((node, ancestor_form)) = pending.pop() {
        let tag = html_local_name(&node);
        let form = (tag.as_deref() == Some("form")).then(|| node.identity());
        if let Some(id) = node.attribute_value_ns_utf16(None, "id") {
            first_ids.entry(id).or_insert(form);
        }
        let kind = if is_submit_button(&node) {
            Some(Kind::Submit)
        } else if tag.as_deref() == Some("input")
            && node
                .attribute_value_ns(None, "type")
                .is_some_and(|value| value.eq_ignore_ascii_case("radio"))
        {
            Some(Kind::Radio {
                name: node
                    .attribute_value_ns_utf16(None, "name")
                    .unwrap_or_default(),
                checked: node.checked(),
            })
        } else {
            None
        };
        if let Some(kind) = kind {
            controls.push(Control {
                node: node.clone(),
                ancestor_form,
                kind,
            });
        }
        for child in node.child_nodes().into_iter().rev() {
            pending.push((child, form.or(ancestor_form)));
        }
    }
    (first_ids, controls)
}

fn owner(
    control: &Control,
    connected: bool,
    first_ids: &HashMap<Vec<u16>, Option<usize>>,
) -> Option<usize> {
    if let Some(owner) = control
        .node
        .peek_parser_form_owner()
        .filter(|owner| html_local_name(owner).as_deref() == Some("form"))
    {
        return Some(owner.identity());
    }
    if connected && let Some(id) = control.node.attribute_value_ns_utf16(None, "form") {
        return if id.is_empty() {
            None
        } else {
            first_ids.get(&id).copied().flatten()
        };
    }
    control.ancestor_form
}

fn build(root: &NodeHandle) -> FormStateIndex {
    let (first_ids, controls) = collect(root);
    let connected = connected(root);
    let mut index = FormStateIndex::default();
    let mut seen_forms = HashSet::new();
    let mut radio_groups = HashMap::<RadioGroup, (bool, Vec<usize>)>::new();
    for control in controls {
        let owner = owner(&control, connected, &first_ids);
        let id = control.node.identity();
        match control.kind {
            Kind::Submit => {
                if let Some(owner) = owner
                    && seen_forms.insert(owner)
                {
                    index.defaults.insert(id);
                }
            }
            Kind::Radio { name, checked } => {
                let key = if name.is_empty() {
                    RadioGroup::Unnamed(id)
                } else {
                    RadioGroup::Named(owner, name)
                };
                let group = radio_groups.entry(key).or_default();
                group.0 |= checked;
                group.1.push(id);
            }
        }
    }
    for (checked, members) in radio_groups.into_values() {
        if !checked {
            index.indeterminate_radios.extend(members);
        }
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_radio_groups_share_one_owned_index() {
        let doc = crate::html::TreeBuilder::parse("<form><button>B</button><input id=a type=radio name=g><input id=b type=radio name=g></form>").document();
        let mut cache = SelectorMatchCache::default();
        let button = doc.query_selector("button").unwrap();
        let a = doc.query_selector("#a").unwrap();
        let b = doc.query_selector("#b").unwrap();
        assert!(super::super::default_state::matches(&button, &mut cache));
        assert!(super::super::indeterminate_state::matches(&a, &mut cache));
        assert!(super::super::indeterminate_state::matches(&b, &mut cache));
        assert_eq!(cache.form_states.len(), 1);
        assert_eq!(cache.form_state_builds, 1);
    }

    #[test]
    fn radio_membership_does_not_retain_removed_dom() {
        let mut cache = SelectorMatchCache::default();
        let weak = {
            let doc = crate::html::TreeBuilder::parse("<form><input type=radio name=g></form>")
                .document();
            let radio = doc.query_selector("input").unwrap();
            assert!(super::super::indeterminate_state::matches(
                &radio, &mut cache
            ));
            doc.downgrade()
        };
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn radio_names_keep_exact_utf16_group_identity() {
        let doc = crate::html::TreeBuilder::parse("<form><input id=a type=radio name=g><input id=b type=radio name=g><input id=c type=radio name=g></form>").document();
        let a = doc.query_selector("#a").unwrap();
        let b = doc.query_selector("#b").unwrap();
        let c = doc.query_selector("#c").unwrap();
        a.set_attribute_value_ns_utf16(None, "name", &[0xd800]);
        b.set_attribute_value_ns_utf16(None, "name", &[0xd800]);
        c.set_attribute_value_ns_utf16(None, "name", &[0xd801]);
        a.set_checked(true);
        let mut cache = SelectorMatchCache::default();
        assert!(!super::super::indeterminate_state::matches(&a, &mut cache));
        assert!(!super::super::indeterminate_state::matches(&b, &mut cache));
        assert!(super::super::indeterminate_state::matches(&c, &mut cache));
    }
}
