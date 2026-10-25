//! Owned form-control grouping shared by parsing and selector evaluation.
use super::{html_local_name, is_submit_button};
use crate::dom::{Node, NodeHandle, NodeType};
use std::collections::{HashMap, HashSet};

/// Owned selector results, retaining numeric identities rather than DOM handles.
#[derive(Debug, Default)]
pub(crate) struct FormStateIndex {
    pub(crate) defaults: HashSet<usize>,
    pub(crate) indeterminate_radios: HashSet<usize>,
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

/// Finds the ordinary tree root without crossing a shadow boundary.
pub(crate) fn tree_root(node: &NodeHandle) -> NodeHandle {
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

/// Computes selector metadata without mutating the tree or retaining its nodes.
pub(crate) fn build(root: &NodeHandle) -> FormStateIndex {
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

/// Parser-owned form reference strings; no DOM handles survive a call.
#[derive(Debug, Default)]
pub(crate) struct ParsedFormReferences {
    ids: HashSet<Vec<u16>>,
}

impl ParsedFormReferences {
    /// Records references and updates peer checkedness after actual insertion.
    pub(crate) fn reconcile(&mut self, node: &NodeHandle) {
        self.observe(node);
        reconcile_inserted_control(node, &self.ids);
    }

    /// Rebuilds references after author code, dropping removed controls' IDs.
    /// Include the active insertion tree in case it was detached or adopted.
    /// Shadow/template trees are visited too; only strings are retained.
    pub(crate) fn refresh(&mut self, document: &NodeHandle, active: &NodeHandle) {
        self.ids.clear();
        let mut pending = vec![document.clone(), tree_root(active)];
        let mut visited = HashSet::new();
        while let Some(node) = pending.pop() {
            if !visited.insert(node.identity()) {
                continue;
            }
            self.observe(&node);
            pending.extend(node.child_nodes());
            if let Some(shadow) = node.shadow_root() {
                pending.push(shadow);
            }
            if let Some(content) = node.template_content() {
                pending.push(content);
            }
        }
    }

    fn observe(&mut self, node: &NodeHandle) {
        let Some(id) = node
            .attribute_value_ns_utf16(None, "form")
            .filter(|id| !id.is_empty())
        else {
            return;
        };
        if html_local_name(node).as_deref() == Some("input")
            && node
                .attribute_value_ns(None, "type")
                .is_some_and(|kind| kind.eq_ignore_ascii_case("radio"))
        {
            self.ids.insert(id);
        }
    }
}

/// Updates checked radio peers after a parser insertion or an ID change.
/// The caller has already inserted the node and assigned its parser form owner.
/// This mutates only checkedness, preserves dirty flags, and retains no nodes.
fn reconcile_inserted_control(node: &NodeHandle, form_ids: &HashSet<Vec<u16>>) {
    let checked_radio = node.input_checkedness()
        && node
            .attribute_value_ns(None, "type")
            .is_some_and(|kind| kind.eq_ignore_ascii_case("radio"));
    let changed_id = node
        .attribute_value_ns_utf16(None, "id")
        .filter(|id| !id.is_empty() && form_ids.contains(id) && can_preempt_existing_id(node));
    if !checked_radio && changed_id.is_none() {
        return;
    }
    let root = tree_root(node);
    let (first_ids, controls) = collect(&root);
    let connected = connected(&root);
    let mut group_ids = HashMap::new();
    let mut groups: Vec<Vec<NodeHandle>> = Vec::new();
    let mut inserted = None;
    let mut references = Vec::new();
    for control in controls {
        let owner = owner(&control, connected, &first_ids);
        let Kind::Radio { name, .. } = control.kind else {
            continue;
        };
        let key = if name.is_empty() {
            RadioGroup::Unnamed(control.node.identity())
        } else {
            RadioGroup::Named(owner, name)
        };
        let group = *group_ids.entry(key).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[group].push(control.node.clone());
        if checked_radio && control.node == *node {
            inserted = Some(group);
        }
        if connected
            && changed_id.as_ref().is_some_and(|id| {
                control.node.attribute_value_ns_utf16(None, "form").as_ref() == Some(id)
            })
        {
            references.push((group, control.node));
        }
    }
    if let Some(group) = inserted {
        select_radio(node, &groups[group]);
    }
    // Reassociation can merge previously distinct groups; later members win.
    for (group, candidate) in references.into_iter().rev() {
        if candidate.input_checkedness() {
            select_radio(&candidate, &groups[group]);
        }
    }
}

fn select_radio(selected: &NodeHandle, group: &[NodeHandle]) {
    for peer in group {
        if peer != selected {
            peer.set_checkedness(false);
        }
    }
}

/// An empty node appended after the existing tree cannot preempt a form ID.
/// Its new form, if any, has no old children to merge into a radio group.
/// Avoid rescanning ordinary pages for every newly parsed element with an ID.
fn can_preempt_existing_id(node: &NodeHandle) -> bool {
    if node.first_child().is_some() {
        return true;
    }
    let mut current = node.clone();
    loop {
        if current.next_sibling().is_some() {
            return true;
        }
        let Some(parent) = current.parent_node() else {
            return false;
        };
        current = parent;
    }
}
