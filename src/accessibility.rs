//! Semantic accessibility tree derived from the composed DOM tree.
//!
//! The accessibility layer deliberately owns no DOM state. A snapshot is
//! rebuilt from stable [`NodeHandle`] identities, live form-control state and a
//! caller-provided renderedness predicate. This keeps script mutations, shadow
//! DOM slotting and stylesheet changes observable without a second invalidation
//! graph.

use std::collections::{HashMap, HashSet};

use crate::dom::{Node, NodeHandle, NodeType, is_actually_disabled};

/// A typed accessibility property value.
#[derive(Debug, Clone, PartialEq)]
pub enum AccessibilityValue {
    Boolean(bool),
    Integer(i64),
    Number(f64),
    String(String),
    Token(String),
    TokenList(String),
    Tristate(String),
    IdRef {
        value: String,
        related_nodes: Vec<AccessibilityRelatedNode>,
    },
    IdRefList {
        value: String,
        related_nodes: Vec<AccessibilityRelatedNode>,
    },
}

/// A DOM node named by an ARIA ID-reference relationship.
#[derive(Debug, Clone, PartialEq)]
pub struct AccessibilityRelatedNode {
    pub dom_node: NodeHandle,
    pub idref: String,
    pub text: String,
}

/// State or relationship exposed on an accessibility node.
#[derive(Debug, Clone, PartialEq)]
pub struct AccessibilityProperty {
    pub name: String,
    pub value: AccessibilityValue,
}

/// How computed style affects an element's accessibility participation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessibilityRenderState {
    Rendered,
    NotRendered,
    NotVisible,
}

/// Live JavaScript-owned state needed for a semantic snapshot.
#[derive(Debug, Clone, Default)]
pub struct AccessibilitySnapshotState {
    pub selected_option_identities: HashSet<usize>,
    pub open_details_identities: HashSet<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HiddenCause {
    AriaElement,
    AriaSubtree,
    InertElement,
    InertSubtree,
    NotRendered,
    NotVisible,
}

impl HiddenCause {
    fn ignored_reason(self) -> &'static str {
        match self {
            Self::AriaElement => "ariaHiddenElement",
            Self::AriaSubtree => "ariaHiddenSubtree",
            Self::InertElement => "inertElement",
            Self::InertSubtree => "inertSubtree",
            Self::NotRendered => "notRendered",
            Self::NotVisible => "notVisible",
        }
    }

    fn descendant_cause(self) -> Self {
        match self {
            Self::AriaElement | Self::AriaSubtree => Self::AriaSubtree,
            Self::InertElement | Self::InertSubtree => Self::InertSubtree,
            Self::NotRendered | Self::NotVisible => self,
        }
    }

    fn prunes_descendants(self) -> bool {
        self != Self::NotVisible
    }
}

/// One node in an accessibility snapshot.
#[derive(Debug, Clone)]
pub struct AccessibilityNode {
    pub node_id: String,
    pub dom_node: NodeHandle,
    pub ignored: bool,
    pub ignored_reasons: Vec<String>,
    pub role: String,
    pub name: String,
    pub description: String,
    pub value: Option<AccessibilityValue>,
    pub properties: Vec<AccessibilityProperty>,
    pub children: Vec<AccessibilityNode>,
}

/// A complete accessibility snapshot rooted at the active document.
#[derive(Debug, Clone)]
pub struct AccessibilityTree {
    pub root: AccessibilityNode,
}

impl AccessibilityTree {
    /// Builds a snapshot from the composed tree.
    ///
    /// `is_rendered` must account for stylesheet-driven `display` and
    /// `visibility`; HTML/ARIA hidden state is handled by this module. The
    /// focused identity is optional because a headless document may have no
    /// explicitly focused element, in which case the root web area is focused.
    pub fn build(
        document: &NodeHandle,
        page_name: impl Into<String>,
        document_generation: u64,
        focused_identity: Option<usize>,
        snapshot_state: &AccessibilitySnapshotState,
        mut render_state: impl FnMut(&NodeHandle) -> AccessibilityRenderState,
    ) -> Self {
        let mut builder = TreeBuilder::new(
            document,
            document_generation,
            focused_identity,
            snapshot_state,
            &mut render_state,
        );
        let root = builder.build_document(document, page_name.into());
        Self { root }
    }

    /// Computes the ignored state for a directly inspected DOM node even when
    /// that node has no object in the normal accessibility tree.
    pub fn build_inspected_node(
        document: &NodeHandle,
        target: &NodeHandle,
        document_generation: u64,
        focused_identity: Option<usize>,
        snapshot_state: &AccessibilitySnapshotState,
        mut render_state: impl FnMut(&NodeHandle) -> AccessibilityRenderState,
    ) -> Option<AccessibilityNode> {
        let mut builder = TreeBuilder::new(
            document,
            document_generation,
            focused_identity,
            snapshot_state,
            &mut render_state,
        );
        builder.retain_pruned_descendants = true;
        let ancestor_hidden = builder.hidden_ancestor_cause(target);
        builder.build_node(target, ancestor_hidden)
    }

    pub fn find_by_dom_identity(&self, identity: usize) -> Option<&AccessibilityNode> {
        find_node(&self.root, &|node| node.dom_node.identity() == identity)
    }

    pub fn find_by_node_id(&self, node_id: &str) -> Option<&AccessibilityNode> {
        find_node(&self.root, &|node| node.node_id == node_id)
    }

    pub fn path_to_dom_identity(&self, identity: usize) -> Option<Vec<&AccessibilityNode>> {
        let mut path = Vec::new();
        if collect_path(&self.root, identity, &mut path) {
            Some(path)
        } else {
            None
        }
    }

    pub fn nodes_preorder(&self) -> Vec<&AccessibilityNode> {
        let mut nodes = Vec::new();
        collect_nodes(&self.root, &mut nodes);
        nodes
    }
}

fn find_node<'a>(
    node: &'a AccessibilityNode,
    predicate: &impl Fn(&AccessibilityNode) -> bool,
) -> Option<&'a AccessibilityNode> {
    if predicate(node) {
        return Some(node);
    }
    node.children
        .iter()
        .find_map(|child| find_node(child, predicate))
}

fn collect_path<'a>(
    node: &'a AccessibilityNode,
    identity: usize,
    path: &mut Vec<&'a AccessibilityNode>,
) -> bool {
    path.push(node);
    if node.dom_node.identity() == identity {
        return true;
    }
    for child in &node.children {
        if collect_path(child, identity, path) {
            return true;
        }
    }
    path.pop();
    false
}

fn collect_nodes<'a>(node: &'a AccessibilityNode, output: &mut Vec<&'a AccessibilityNode>) {
    output.push(node);
    for child in &node.children {
        collect_nodes(child, output);
    }
}

fn collect_ax_dom_nodes(node: &AccessibilityNode, output: &mut Vec<NodeHandle>) {
    output.push(node.dom_node.clone());
    for child in &node.children {
        collect_ax_dom_nodes(child, output);
    }
}

fn find_ax_node(node: &AccessibilityNode, identity: usize) -> Option<&AccessibilityNode> {
    if node.dom_node.identity() == identity {
        return Some(node);
    }
    node.children
        .iter()
        .find_map(|child| find_ax_node(child, identity))
}

fn find_ax_node_mut(
    node: &mut AccessibilityNode,
    identity: usize,
) -> Option<&mut AccessibilityNode> {
    if node.dom_node.identity() == identity {
        return Some(node);
    }
    node.children
        .iter_mut()
        .find_map(|child| find_ax_node_mut(child, identity))
}

fn contains_ax_identity(node: &AccessibilityNode, identity: usize) -> bool {
    find_ax_node(node, identity).is_some()
}

fn detach_ax_node(parent: &mut AccessibilityNode, identity: usize) -> Option<AccessibilityNode> {
    if let Some(index) = parent
        .children
        .iter()
        .position(|child| child.dom_node.identity() == identity)
    {
        return Some(parent.children.remove(index));
    }
    parent
        .children
        .iter_mut()
        .find_map(|child| detach_ax_node(child, identity))
}

struct TreeBuilder<'a, F> {
    document_identity: usize,
    document_generation: u64,
    focused_identity: Option<usize>,
    snapshot_state: &'a AccessibilitySnapshotState,
    render_state: &'a mut F,
    ids: HashMap<usize, HashMap<String, NodeHandle>>,
    labels: HashMap<usize, Vec<NodeHandle>>,
    retain_pruned_descendants: bool,
}

impl<'a, F> TreeBuilder<'a, F>
where
    F: FnMut(&NodeHandle) -> AccessibilityRenderState,
{
    fn new(
        document: &NodeHandle,
        document_generation: u64,
        focused_identity: Option<usize>,
        snapshot_state: &'a AccessibilitySnapshotState,
        render_state: &'a mut F,
    ) -> Self {
        let document_identity = document.identity();
        let mut ids = HashMap::new();
        let mut labels = HashMap::new();
        index_dom(document, document_identity, &mut ids, &mut labels);
        Self {
            document_identity,
            document_generation,
            focused_identity,
            snapshot_state,
            render_state,
            ids,
            labels,
            retain_pruned_descendants: false,
        }
    }

    fn build_document(&mut self, document: &NodeHandle, page_name: String) -> AccessibilityNode {
        let mut children = Vec::new();
        for child in document.layout_child_nodes() {
            if let Some(child) = self.build_node(&child, None) {
                children.push(child);
            }
        }
        let mut root = AccessibilityNode {
            node_id: ax_node_id(document, self.document_generation),
            dom_node: document.clone(),
            ignored: false,
            ignored_reasons: Vec::new(),
            role: "RootWebArea".to_string(),
            name: page_name,
            description: String::new(),
            value: None,
            properties: vec![AccessibilityProperty {
                name: "focused".to_string(),
                value: AccessibilityValue::Boolean(self.focused_identity.is_none()),
            }],
            children,
        };
        self.apply_aria_owns(&mut root);
        root
    }

    fn tree_scope_id(&self, node: &NodeHandle) -> usize {
        node.containing_shadow_root()
            .map(|root| root.identity())
            .unwrap_or(self.document_identity)
    }

    fn id_target(&self, node: &NodeHandle, id: &str) -> Option<NodeHandle> {
        self.ids
            .get(&self.tree_scope_id(node))
            .and_then(|ids| ids.get(id))
            .cloned()
    }

    fn labels_for(&self, node: &NodeHandle) -> Vec<NodeHandle> {
        self.labels
            .get(&self.tree_scope_id(node))
            .cloned()
            .unwrap_or_default()
    }

    fn apply_aria_owns(&mut self, root: &mut AccessibilityNode) {
        let mut owners = Vec::new();
        collect_ax_dom_nodes(root, &mut owners);
        let mut ownership = Vec::new();
        for owner in owners {
            if !allows_aria_ownership(&owner) {
                continue;
            }
            let Some(idrefs) = owner.get_attribute("aria-owns") else {
                continue;
            };
            for idref in idrefs.split_ascii_whitespace() {
                if let Some(target) = self.id_target(&owner, idref) {
                    ownership.push((owner.clone(), target));
                }
            }
        }

        let mut claimed = HashSet::new();
        for (owner, target) in ownership {
            let owner_identity = owner.identity();
            let target_identity = target.identity();
            if owner_identity == target_identity || claimed.contains(&target_identity) {
                continue;
            }
            if find_ax_node(root, owner_identity).is_none() {
                continue;
            }
            if dom_contains_identity(&target, owner_identity) {
                continue;
            }
            if find_ax_node(root, target_identity)
                .is_some_and(|target| contains_ax_identity(target, owner_identity))
            {
                continue;
            }
            let target_node = if let Some(target) = detach_ax_node(root, target_identity) {
                target
            } else {
                let ancestor_hidden = self.hidden_ancestor_cause(&target);
                let Some(target) = self.build_node(&target, ancestor_hidden) else {
                    continue;
                };
                target
            };
            let Some(owner) = find_ax_node_mut(root, owner_identity) else {
                continue;
            };
            claimed.insert(target_identity);
            owner.children.push(target_node);
        }
    }

    fn hidden_ancestor_cause(&mut self, node: &NodeHandle) -> Option<HiddenCause> {
        let mut current = composed_parent(node);
        let check_inherited_visibility = node.node_type() == NodeType::Text;
        let mut resolved_render_state = false;
        while let Some(ancestor) = current {
            if ancestor.node_type() == NodeType::Element {
                if ancestor
                    .get_attribute("aria-hidden")
                    .is_some_and(|value| value.eq_ignore_ascii_case("true"))
                {
                    return Some(HiddenCause::AriaSubtree);
                }
                if ancestor.get_attribute("inert").is_some() {
                    return Some(HiddenCause::InertSubtree);
                }
                let tag = ancestor.tag_name().unwrap_or_default();
                if ancestor.get_attribute("hidden").is_some()
                    || (tag == "input" && input_type(&ancestor) == "hidden")
                    || matches!(
                        tag.as_str(),
                        "head"
                            | "base"
                            | "link"
                            | "meta"
                            | "noscript"
                            | "script"
                            | "style"
                            | "template"
                            | "title"
                            | "source"
                    )
                {
                    return Some(HiddenCause::NotRendered);
                }
                // The render-state resolver includes ancestor display and
                // inherited visibility, so resolving the closest element once
                // avoids repeating its ancestor walk at every level here.
                if !resolved_render_state {
                    match (self.render_state)(&ancestor) {
                        AccessibilityRenderState::NotRendered => {
                            return Some(HiddenCause::NotRendered);
                        }
                        AccessibilityRenderState::NotVisible if check_inherited_visibility => {
                            return Some(HiddenCause::NotVisible);
                        }
                        AccessibilityRenderState::Rendered
                        | AccessibilityRenderState::NotVisible => {}
                    }
                    resolved_render_state = true;
                }
            }
            current = composed_parent(&ancestor);
        }
        None
    }

    fn build_node(
        &mut self,
        node: &NodeHandle,
        ancestor_hidden: Option<HiddenCause>,
    ) -> Option<AccessibilityNode> {
        match node.node_type() {
            NodeType::Text => self.build_text(node, ancestor_hidden),
            NodeType::Element => self.build_element(node, ancestor_hidden),
            NodeType::Document => Some(self.build_document(node, document_title(node))),
            NodeType::DocumentFragment => {
                // Shadow roots and slots are transparent in the composed tree.
                None
            }
            NodeType::Comment | NodeType::ProcessingInstruction | NodeType::DocumentType => None,
        }
    }

    fn build_text(
        &mut self,
        node: &NodeHandle,
        ancestor_hidden: Option<HiddenCause>,
    ) -> Option<AccessibilityNode> {
        if ancestor_hidden.is_some() && !self.retain_pruned_descendants {
            return None;
        }
        let text = normalize_whitespace(&node.data().unwrap_or_default());
        if text.is_empty() {
            return None;
        }
        Some(AccessibilityNode {
            node_id: ax_node_id(node, self.document_generation),
            dom_node: node.clone(),
            ignored: ancestor_hidden.is_some(),
            ignored_reasons: ancestor_hidden
                .map(|cause| vec![cause.ignored_reason().to_string()])
                .unwrap_or_default(),
            role: "StaticText".to_string(),
            name: text,
            description: String::new(),
            value: None,
            properties: Vec::new(),
            children: Vec::new(),
        })
    }

    fn build_element(
        &mut self,
        node: &NodeHandle,
        ancestor_hidden: Option<HiddenCause>,
    ) -> Option<AccessibilityNode> {
        let tag = node.tag_name().unwrap_or_default();
        let aria_hidden = node
            .get_attribute("aria-hidden")
            .is_some_and(|value| value.eq_ignore_ascii_case("true"));
        let inert = node.get_attribute("inert").is_some();
        let html_hidden = node.get_attribute("hidden").is_some()
            || (tag == "input"
                && node
                    .get_attribute("type")
                    .is_some_and(|value| value.eq_ignore_ascii_case("hidden")));
        let structurally_hidden = matches!(
            tag.as_str(),
            "head"
                | "base"
                | "link"
                | "meta"
                | "noscript"
                | "script"
                | "style"
                | "template"
                | "title"
                | "source"
        );
        let render_state = (self.render_state)(node);
        let hidden_cause = ancestor_hidden.or({
            if aria_hidden {
                Some(HiddenCause::AriaElement)
            } else if inert {
                Some(HiddenCause::InertElement)
            } else if html_hidden || structurally_hidden {
                Some(HiddenCause::NotRendered)
            } else {
                match render_state {
                    AccessibilityRenderState::Rendered => None,
                    AccessibilityRenderState::NotRendered => Some(HiddenCause::NotRendered),
                    AccessibilityRenderState::NotVisible => Some(HiddenCause::NotVisible),
                }
            }
        });
        let retained_aria_owner = allows_aria_ownership(node)
            && node.get_attribute("aria-owns").is_some_and(|idrefs| {
                idrefs
                    .split_ascii_whitespace()
                    .any(|idref| self.id_target(node, idref).is_some())
            });
        if hidden_cause == Some(HiddenCause::NotRendered)
            && !self.retain_pruned_descendants
            && !retained_aria_owner
        {
            return None;
        }

        let (role, presentation, semantic) = computed_role(node);
        let name = self.accessible_name(node, &role, &mut HashSet::new());
        let description = self.accessible_description(node, &name);
        let explicitly_named = !role_prohibits_name(&role)
            && (node.get_attribute("aria-label").is_some()
                || node.get_attribute("aria-labelledby").is_some());
        let has_effective_global_aria = has_global_aria_attribute_for_role(node, &role);
        let empty_alt = tag == "img"
            && node
                .get_attribute("alt")
                .is_some_and(|alt| alt.trim().is_empty())
            && node.get_attribute("role").is_none()
            && !is_focusable(node)
            && !has_effective_global_aria;
        let ignored = hidden_cause.is_some()
            || empty_alt
            || presentation
            || (!semantic
                && !explicitly_named
                && !is_focusable(node)
                && !has_effective_global_aria);
        let ignored_reasons = if let Some(cause) = hidden_cause {
            vec![cause.ignored_reason().to_string()]
        } else if empty_alt {
            vec!["emptyAlt".to_string()]
        } else if presentation {
            vec!["presentationalRole".to_string()]
        } else if ignored {
            vec!["uninteresting".to_string()]
        } else {
            Vec::new()
        };

        let mut children = Vec::new();
        if hidden_cause.is_none()
            || hidden_cause.is_some_and(|cause| !cause.prunes_descendants())
            || self.retain_pruned_descendants
        {
            let descendant_hidden = hidden_cause
                .filter(|cause| cause.prunes_descendants())
                .map(HiddenCause::descendant_cause);
            for child in node.layout_child_nodes() {
                let child_hidden = if hidden_cause == Some(HiddenCause::NotVisible)
                    && child.node_type() != NodeType::Element
                {
                    Some(HiddenCause::NotVisible)
                } else {
                    descendant_hidden
                };
                if let Some(child) = self.build_node(&child, child_hidden) {
                    children.push(child);
                }
            }
        }

        Some(AccessibilityNode {
            node_id: ax_node_id(node, self.document_generation),
            dom_node: node.clone(),
            ignored,
            ignored_reasons,
            role,
            name,
            description,
            value: accessibility_value(node, self.snapshot_state),
            properties: self.accessibility_properties(node),
            children,
        })
    }

    fn hidden_cause_for_name(&mut self, node: &NodeHandle) -> Option<HiddenCause> {
        if node.node_type() != NodeType::Element {
            return None;
        }
        let tag = node.tag_name().unwrap_or_default();
        if node
            .get_attribute("aria-hidden")
            .is_some_and(|value| value.eq_ignore_ascii_case("true"))
        {
            return Some(HiddenCause::AriaElement);
        }
        if node.get_attribute("inert").is_some() {
            return Some(HiddenCause::InertElement);
        }
        if node.get_attribute("hidden").is_some()
            || (tag == "input" && input_type(node) == "hidden")
            || matches!(
                tag.as_str(),
                "head"
                    | "base"
                    | "link"
                    | "meta"
                    | "noscript"
                    | "script"
                    | "style"
                    | "template"
                    | "title"
                    | "source"
            )
        {
            return Some(HiddenCause::NotRendered);
        }
        match (self.render_state)(node) {
            AccessibilityRenderState::Rendered => None,
            AccessibilityRenderState::NotRendered => Some(HiddenCause::NotRendered),
            AccessibilityRenderState::NotVisible => Some(HiddenCause::NotVisible),
        }
    }

    fn node_hidden_for_name(&mut self, node: &NodeHandle) -> bool {
        self.hidden_cause_for_name(node).is_some() || self.hidden_ancestor_cause(node).is_some()
    }

    fn embedded_control_text(&self, node: &NodeHandle) -> Option<String> {
        let role = computed_role(node).0;
        if !matches!(
            role.as_str(),
            "textbox"
                | "searchbox"
                | "combobox"
                | "listbox"
                | "slider"
                | "spinbutton"
                | "scrollbar"
                | "progressbar"
        ) {
            return None;
        }
        match accessibility_value(node, self.snapshot_state)? {
            AccessibilityValue::Integer(value) => Some(value.to_string()),
            AccessibilityValue::Number(value) => Some(value.to_string()),
            AccessibilityValue::String(value)
            | AccessibilityValue::Token(value)
            | AccessibilityValue::TokenList(value)
            | AccessibilityValue::Tristate(value) => Some(value),
            AccessibilityValue::Boolean(value) => Some(value.to_string()),
            AccessibilityValue::IdRef { .. } | AccessibilityValue::IdRefList { .. } => None,
        }
    }

    fn text_alternative(
        &mut self,
        node: &NodeHandle,
        visited: &mut HashSet<usize>,
        include_hidden: bool,
    ) -> String {
        let visibility_hidden = if include_hidden {
            false
        } else {
            match self.hidden_cause_for_name(node) {
                Some(HiddenCause::NotVisible) => true,
                Some(_) => return String::new(),
                None => false,
            }
        };
        if node.node_type() == NodeType::Text {
            return normalize_whitespace(&node.data().unwrap_or_default());
        }
        if let Some(value) = self.embedded_control_text(node) {
            return normalize_whitespace(&value);
        }
        if visited.contains(&node.identity()) {
            return String::new();
        }
        if node.node_type() == NodeType::Element
            && (node.get_attribute("aria-labelledby").is_some()
                || node
                    .get_attribute("aria-label")
                    .is_some_and(|label| !label.trim().is_empty()))
        {
            let role = computed_role(node).0;
            let mut branch_visited = visited.clone();
            let name = self.accessible_name(node, &role, &mut branch_visited);
            if !name.is_empty() {
                return name;
            }
        }
        if !visited.insert(node.identity()) {
            return String::new();
        }
        let tag = node.tag_name().unwrap_or_default();
        if matches!(tag.as_str(), "script" | "style" | "template" | "noscript") {
            return String::new();
        }
        if tag == "img" {
            return normalize_whitespace(&node.get_attribute("alt").unwrap_or_default());
        }
        if tag == "input" {
            let kind = input_type(node);
            if matches!(kind.as_str(), "button" | "submit" | "reset") {
                return normalize_whitespace(&node.get_attribute("value").unwrap_or_default());
            }
        }
        let text = node
            .layout_child_nodes()
            .iter()
            .filter(|child| !(visibility_hidden && child.node_type() == NodeType::Text))
            .map(|child| self.text_alternative(child, visited, include_hidden))
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        normalize_whitespace(&text)
    }

    fn descendant_text_alternative(
        &mut self,
        node: &NodeHandle,
        visited: &mut HashSet<usize>,
        include_hidden: bool,
    ) -> String {
        let text = node
            .layout_child_nodes()
            .iter()
            .map(|child| self.text_alternative(child, visited, include_hidden))
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        normalize_whitespace(&text)
    }

    fn accessible_name(
        &mut self,
        node: &NodeHandle,
        role: &str,
        visited: &mut HashSet<usize>,
    ) -> String {
        self.accessible_name_internal(node, role, visited, false)
    }

    fn accessible_name_internal(
        &mut self,
        node: &NodeHandle,
        role: &str,
        visited: &mut HashSet<usize>,
        in_labelledby_traversal: bool,
    ) -> String {
        if role_prohibits_name(role) {
            return String::new();
        }
        if !visited.insert(node.identity()) {
            return String::new();
        }

        if !in_labelledby_traversal && let Some(references) = node.get_attribute("aria-labelledby")
        {
            let mut names = Vec::new();
            for id in references.split_ascii_whitespace() {
                let Some(label) = self.id_target(node, id) else {
                    continue;
                };
                let label_role = computed_role(&label).0;
                let mut branch_visited = visited.clone();
                let include_hidden = self.node_hidden_for_name(&label);
                let name =
                    self.accessible_name_internal(&label, &label_role, &mut branch_visited, true);
                let name = if name.is_empty() {
                    self.descendant_text_alternative(&label, &mut branch_visited, include_hidden)
                } else {
                    name
                };
                if !name.is_empty() {
                    names.push(name);
                }
            }
            if !names.is_empty() {
                return normalize_whitespace(&names.join(" "));
            }
        }
        if let Some(label) = node.get_attribute("aria-label") {
            let label = normalize_whitespace(&label);
            if !label.is_empty() {
                return label;
            }
        }

        let tag = node.tag_name().unwrap_or_default();
        if is_labelable(node) {
            let id = node.get_attribute("id");
            let mut labels = Vec::new();
            for label in self.labels_for(node) {
                if label_owns_control(&label, node, id.as_deref()) {
                    let include_hidden = self.node_hidden_for_name(&label);
                    let name = self.text_alternative(&label, &mut HashSet::new(), include_hidden);
                    if !name.is_empty() {
                        labels.push(name);
                    }
                }
            }
            if !labels.is_empty() {
                return normalize_whitespace(&labels.join(" "));
            }
        }

        match tag.as_str() {
            "img" | "area" => {
                if let Some(alt) = node.get_attribute("alt") {
                    return normalize_whitespace(&alt);
                }
            }
            "input" => {
                let kind = input_type(node);
                if kind == "image"
                    && let Some(alt) = node.get_attribute("alt")
                {
                    return normalize_whitespace(&alt);
                }
                if matches!(kind.as_str(), "button" | "submit" | "reset") {
                    if let Some(value) = node.get_attribute("value")
                        && !value.is_empty()
                    {
                        return normalize_whitespace(&value);
                    }
                    return match kind.as_str() {
                        "submit" => "Submit".to_string(),
                        "reset" => "Reset".to_string(),
                        _ => String::new(),
                    };
                }
            }
            "fieldset" => {
                if let Some(legend) = node
                    .child_nodes()
                    .into_iter()
                    .find(|child| child.tag_name().as_deref() == Some("legend"))
                {
                    let include_hidden = self.node_hidden_for_name(&legend);
                    return self.text_alternative(&legend, &mut HashSet::new(), include_hidden);
                }
            }
            "figure" => {
                if let Some(caption) = node
                    .child_nodes()
                    .into_iter()
                    .find(|child| child.tag_name().as_deref() == Some("figcaption"))
                {
                    let include_hidden = self.node_hidden_for_name(&caption);
                    return self.text_alternative(&caption, &mut HashSet::new(), include_hidden);
                }
            }
            "table" => {
                if let Some(caption) = node
                    .child_nodes()
                    .into_iter()
                    .find(|child| child.tag_name().as_deref() == Some("caption"))
                {
                    let include_hidden = self.node_hidden_for_name(&caption);
                    return self.text_alternative(&caption, &mut HashSet::new(), include_hidden);
                }
            }
            _ => {}
        }

        if role_allows_name_from_content(role) {
            let include_hidden = self.node_hidden_for_name(node);
            let content = node
                .layout_child_nodes()
                .iter()
                .map(|child| self.text_alternative(child, &mut HashSet::new(), include_hidden))
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            let content = normalize_whitespace(&content);
            if !content.is_empty() {
                return content;
            }
        }
        if let Some(title) = node.get_attribute("title") {
            return normalize_whitespace(&title);
        }
        if matches!(role, "textbox" | "searchbox")
            && let Some(placeholder) = node.get_attribute("placeholder")
        {
            return normalize_whitespace(&placeholder);
        }
        String::new()
    }

    fn accessible_description(&mut self, node: &NodeHandle, name: &str) -> String {
        if let Some(references) = node.get_attribute("aria-describedby") {
            let targets = references
                .split_ascii_whitespace()
                .filter_map(|id| self.id_target(node, id))
                .collect::<Vec<_>>();
            let mut descriptions = Vec::new();
            for target in targets {
                let text = self.text_alternative(&target, &mut HashSet::new(), true);
                if !text.is_empty() {
                    descriptions.push(text);
                }
            }
            let description = descriptions.join(" ");
            if !description.is_empty() {
                return normalize_whitespace(&description);
            }
        }
        node.get_attribute("title")
            .map(|title| normalize_whitespace(&title))
            .filter(|title| title != name)
            .unwrap_or_default()
    }

    fn accessibility_properties(&mut self, node: &NodeHandle) -> Vec<AccessibilityProperty> {
        let mut properties =
            accessibility_properties(node, self.focused_identity, self.snapshot_state);
        for (attribute, property, single) in [
            ("aria-activedescendant", "activedescendant", true),
            ("aria-controls", "controls", false),
            ("aria-describedby", "describedby", false),
            ("aria-details", "details", true),
            ("aria-errormessage", "errormessage", true),
            ("aria-flowto", "flowto", false),
            ("aria-labelledby", "labelledby", false),
            ("aria-owns", "owns", false),
        ] {
            let Some(raw_value) = node.get_attribute(attribute) else {
                continue;
            };
            let targets = raw_value
                .split_ascii_whitespace()
                .filter_map(|idref| {
                    self.id_target(node, idref)
                        .map(|target| (idref.to_string(), target))
                })
                .collect::<Vec<_>>();
            let mut references = Vec::new();
            for (idref, target) in targets {
                references.push(AccessibilityRelatedNode {
                    dom_node: target.clone(),
                    idref,
                    text: self.text_alternative(&target, &mut HashSet::new(), true),
                });
            }
            if references.is_empty() {
                continue;
            }
            let value = references
                .iter()
                .map(|reference| reference.idref.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            let value = if single {
                AccessibilityValue::IdRef {
                    value,
                    related_nodes: references,
                }
            } else {
                AccessibilityValue::IdRefList {
                    value,
                    related_nodes: references,
                }
            };
            properties.push(AccessibilityProperty {
                name: property.to_string(),
                value,
            });
        }
        properties
    }
}

fn ax_node_id(node: &NodeHandle, document_generation: u64) -> String {
    format!("ax-{document_generation}-{}", node.identity())
}

fn composed_parent(node: &NodeHandle) -> Option<NodeHandle> {
    node.assigned_slot().or_else(|| {
        node.parent_node().and_then(|parent| {
            if parent.node_type() == NodeType::DocumentFragment {
                parent.shadow_host()
            } else {
                Some(parent)
            }
        })
    })
}

fn dom_contains_identity(root: &NodeHandle, identity: usize) -> bool {
    if root.identity() == identity {
        return true;
    }
    if let Some(shadow_root) = root.shadow_root()
        && dom_contains_identity(&shadow_root, identity)
    {
        return true;
    }
    root.child_nodes()
        .iter()
        .any(|child| dom_contains_identity(child, identity))
}

fn allows_aria_ownership(node: &NodeHandle) -> bool {
    if matches!(node.tag_name().as_deref(), Some("input" | "textarea"))
        || node
            .get_attribute("contenteditable")
            .is_some_and(|value| value.is_empty() || value.eq_ignore_ascii_case("true"))
    {
        return false;
    }
    !matches!(
        computed_role(node).0.as_str(),
        "button"
            | "checkbox"
            | "image"
            | "link"
            | "meter"
            | "option"
            | "progressbar"
            | "radio"
            | "scrollbar"
            | "searchbox"
            | "separator"
            | "slider"
            | "spinbutton"
            | "switch"
            | "tab"
            | "textbox"
    )
}

fn index_dom(
    node: &NodeHandle,
    scope_id: usize,
    ids: &mut HashMap<usize, HashMap<String, NodeHandle>>,
    labels: &mut HashMap<usize, Vec<NodeHandle>>,
) {
    if node.node_type() == NodeType::Element {
        if let Some(id) = node.get_attribute("id")
            && !id.is_empty()
        {
            ids.entry(scope_id)
                .or_default()
                .entry(id)
                .or_insert_with(|| node.clone());
        }
        if node.tag_name().as_deref() == Some("label") {
            labels.entry(scope_id).or_default().push(node.clone());
        }
        if let Some(root) = node.shadow_root() {
            index_dom(&root, root.identity(), ids, labels);
        }
    }
    for child in node.child_nodes() {
        index_dom(&child, scope_id, ids, labels);
    }
}

fn document_title(document: &NodeHandle) -> String {
    find_descendant(document, &|node| {
        node.tag_name().as_deref() == Some("title")
    })
    .map(|title| text_alternative(&title, &mut HashSet::new()))
    .unwrap_or_default()
}

fn find_descendant(
    node: &NodeHandle,
    predicate: &impl Fn(&NodeHandle) -> bool,
) -> Option<NodeHandle> {
    for child in node.child_nodes() {
        if predicate(&child) {
            return Some(child);
        }
        if let Some(found) = find_descendant(&child, predicate) {
            return Some(found);
        }
    }
    None
}

fn text_alternative(node: &NodeHandle, visited: &mut HashSet<usize>) -> String {
    if !visited.insert(node.identity()) {
        return String::new();
    }
    if node.node_type() == NodeType::Text {
        return normalize_whitespace(&node.data().unwrap_or_default());
    }
    let tag = node.tag_name().unwrap_or_default();
    if matches!(tag.as_str(), "script" | "style" | "template" | "noscript") {
        return String::new();
    }
    if tag == "img" {
        return normalize_whitespace(&node.get_attribute("alt").unwrap_or_default());
    }
    if tag == "input" {
        let kind = input_type(node);
        if matches!(kind.as_str(), "button" | "submit" | "reset") {
            return normalize_whitespace(&node.get_attribute("value").unwrap_or_default());
        }
    }
    let text = node
        .layout_child_nodes()
        .iter()
        .map(|child| text_alternative(child, visited))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    normalize_whitespace(&text)
}

fn normalize_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn computed_role(node: &NodeHandle) -> (String, bool, bool) {
    if let Some(raw) = node.get_attribute("role") {
        for token in raw.split_ascii_whitespace() {
            if let Some(role) = explicit_role(token) {
                let presentation = matches!(role, "none" | "presentation");
                // WAI-ARIA conflict resolution ignores a presentational role
                // when the element can receive focus or carries a global ARIA
                // state/property; its implicit native role remains exposed.
                if presentation
                    && (is_focusable(node) || has_global_aria_attribute_for_role(node, role))
                {
                    break;
                }
                return (role.to_string(), presentation, !presentation);
            }
        }
    }

    let tag = node.tag_name().unwrap_or_default();
    let role = match tag.as_str() {
        "a" | "area" if node.get_attribute("href").is_some() => "link",
        "article" => "article",
        "aside" => "complementary",
        "audio" => "audio",
        "button" => "button",
        "canvas" => "Canvas",
        "caption" => "caption",
        "code" => "code",
        "dd" => "definition",
        "details" | "fieldset" => "group",
        "dialog" => "dialog",
        "dl" => "DescriptionList",
        "dt" => "term",
        "figure" => "figure",
        "footer" => "contentinfo",
        "form" => "form",
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => "heading",
        "header" => "banner",
        "hr" => "separator",
        "iframe" => "Iframe",
        "img" => "image",
        "input" => match input_type(node).as_str() {
            "button" | "submit" | "reset" | "image" => "button",
            "checkbox" => "checkbox",
            "radio" => "radio",
            "range" => "slider",
            "number" => "spinbutton",
            "search" => "searchbox",
            _ => "textbox",
        },
        "label" | "legend" => "LabelText",
        "li" => "listitem",
        "main" => "main",
        "mark" => "mark",
        "meter" => "meter",
        "nav" => "navigation",
        "object" | "embed" => "EmbeddedObject",
        "ol" | "ul" => "list",
        "option" => "option",
        "output" => "status",
        "p" => "paragraph",
        "progress" => "progressbar",
        "select"
            if node.get_attribute("multiple").is_some()
                || node
                    .get_attribute("size")
                    .and_then(|size| size.parse::<u64>().ok())
                    .is_some_and(|size| size > 1) =>
        {
            "listbox"
        }
        "select" => "combobox",
        "strong" => "strong",
        "em" => "emphasis",
        "summary" => "button",
        "svg" => "SvgRoot",
        "table" => "table",
        "tbody" | "thead" | "tfoot" => "rowgroup",
        "td" => "cell",
        "textarea" => "textbox",
        "th" if node.get_attribute("scope").is_some_and(|scope| {
            scope.eq_ignore_ascii_case("row") || scope.eq_ignore_ascii_case("rowgroup")
        }) =>
        {
            "rowheader"
        }
        "th" => "columnheader",
        "time" => "time",
        "tr" => "row",
        "video" => "Video",
        _ => "generic",
    };
    (role.to_string(), false, role != "generic")
}

fn explicit_role(role: &str) -> Option<&'static str> {
    let role = role.to_ascii_lowercase();
    const ROLES: &[&str] = &[
        "alert",
        "alertdialog",
        "application",
        "article",
        "banner",
        "blockquote",
        "button",
        "caption",
        "cell",
        "checkbox",
        "code",
        "columnheader",
        "combobox",
        "complementary",
        "contentinfo",
        "definition",
        "deletion",
        "dialog",
        "directory",
        "document",
        "emphasis",
        "feed",
        "figure",
        "form",
        "generic",
        "grid",
        "gridcell",
        "group",
        "heading",
        "insertion",
        "link",
        "list",
        "listbox",
        "listitem",
        "log",
        "main",
        "marquee",
        "math",
        "menu",
        "menubar",
        "menuitem",
        "menuitemcheckbox",
        "menuitemradio",
        "meter",
        "navigation",
        "none",
        "note",
        "option",
        "paragraph",
        "presentation",
        "progressbar",
        "radio",
        "radiogroup",
        "region",
        "row",
        "rowgroup",
        "rowheader",
        "scrollbar",
        "search",
        "searchbox",
        "separator",
        "slider",
        "spinbutton",
        "status",
        "strong",
        "subscript",
        "suggestion",
        "superscript",
        "switch",
        "tab",
        "table",
        "tablist",
        "tabpanel",
        "term",
        "textbox",
        "time",
        "timer",
        "toolbar",
        "tooltip",
        "tree",
        "treegrid",
        "treeitem",
    ];
    if role == "img" {
        return Some("image");
    }
    ROLES.iter().copied().find(|candidate| *candidate == role)
}

fn role_allows_name_from_content(role: &str) -> bool {
    matches!(
        role,
        "button"
            | "cell"
            | "checkbox"
            | "columnheader"
            | "heading"
            | "LabelText"
            | "link"
            | "listitem"
            | "menuitem"
            | "menuitemcheckbox"
            | "menuitemradio"
            | "option"
            | "radio"
            | "rowheader"
            | "switch"
            | "tab"
            | "term"
            | "treeitem"
    )
}

fn role_prohibits_name(role: &str) -> bool {
    matches!(
        role,
        "caption"
            | "code"
            | "deletion"
            | "emphasis"
            | "generic"
            | "insertion"
            | "paragraph"
            | "presentation"
            | "none"
            | "strong"
            | "subscript"
            | "superscript"
    )
}

fn input_type(node: &NodeHandle) -> String {
    node.get_attribute("type")
        .unwrap_or_else(|| "text".to_string())
        .to_ascii_lowercase()
}

fn is_labelable(node: &NodeHandle) -> bool {
    matches!(
        node.tag_name().as_deref(),
        Some("button" | "input" | "meter" | "output" | "progress" | "select" | "textarea")
    ) && !(node.tag_name().as_deref() == Some("input") && input_type(node) == "hidden")
}

fn label_owns_control(label: &NodeHandle, control: &NodeHandle, control_id: Option<&str>) -> bool {
    if let Some(target) = label.get_attribute("for") {
        return control_id.is_some_and(|id| target == id);
    }
    let mut current = control.parent_node();
    while let Some(node) = current {
        if &node == label {
            return true;
        }
        current = node.parent_node();
    }
    false
}

fn accessibility_value(
    node: &NodeHandle,
    snapshot_state: &AccessibilitySnapshotState,
) -> Option<AccessibilityValue> {
    let tag = node.tag_name()?;
    let role = computed_role(node).0;
    if matches!(
        role.as_str(),
        "slider" | "spinbutton" | "scrollbar" | "progressbar"
    ) {
        if let Some(value) = node.get_attribute("aria-valuetext") {
            return Some(AccessibilityValue::String(value));
        }
        if let Some(value) = node
            .get_attribute("aria-valuenow")
            .and_then(|value| value.parse::<f64>().ok())
        {
            return Some(AccessibilityValue::Number(value));
        }
    }
    match tag.as_str() {
        "input" => {
            let kind = input_type(node);
            if matches!(
                kind.as_str(),
                "checkbox" | "radio" | "button" | "submit" | "reset" | "image"
            ) {
                None
            } else {
                let value = node
                    .text_control_state()
                    .map(|state| state.value)
                    .or_else(|| node.get_attribute("value"))
                    .unwrap_or_default();
                let value = if kind == "password" {
                    "•".repeat(value.encode_utf16().count())
                } else {
                    value
                };
                Some(AccessibilityValue::String(value))
            }
        }
        "textarea" => Some(AccessibilityValue::String(
            node.text_control_state()
                .map(|state| state.value)
                .unwrap_or_else(|| text_alternative(node, &mut HashSet::new())),
        )),
        "select" => {
            let selected = find_descendant(node, &|candidate| {
                candidate.tag_name().as_deref() == Some("option")
                    && snapshot_state
                        .selected_option_identities
                        .contains(&candidate.identity())
            });
            selected.map(|option| {
                AccessibilityValue::String(text_alternative(&option, &mut HashSet::new()))
            })
        }
        "progress" | "meter" => node
            .get_attribute("value")
            .and_then(|value| value.parse::<f64>().ok())
            .map(AccessibilityValue::Number),
        _ => node
            .get_attribute("aria-valuetext")
            .map(AccessibilityValue::String)
            .or_else(|| {
                node.get_attribute("aria-valuenow")
                    .and_then(|value| value.parse::<f64>().ok())
                    .map(AccessibilityValue::Number)
            }),
    }
}

fn accessibility_properties(
    node: &NodeHandle,
    focused_identity: Option<usize>,
    snapshot_state: &AccessibilitySnapshotState,
) -> Vec<AccessibilityProperty> {
    let mut properties = Vec::new();
    let tag = node.tag_name().unwrap_or_default();
    let role = computed_role(node).0;

    if is_actually_disabled(node) || node.get_attribute("aria-disabled").is_some() {
        push_boolean(
            &mut properties,
            "disabled",
            is_actually_disabled(node)
                || aria_boolean(node.get_attribute("aria-disabled")).unwrap_or(false),
        );
    }
    if is_focusable(node) {
        push_boolean(&mut properties, "focusable", !is_actually_disabled(node));
    }
    if focused_identity == Some(node.identity()) {
        push_boolean(&mut properties, "focused", true);
    }
    if matches!(
        role.as_str(),
        "checkbox" | "radio" | "switch" | "menuitemcheckbox" | "menuitemradio"
    ) {
        let checked = node
            .get_attribute("aria-checked")
            .map(|value| value.to_ascii_lowercase())
            .unwrap_or_else(|| node.checked().to_string());
        properties.push(AccessibilityProperty {
            name: "checked".to_string(),
            value: AccessibilityValue::Tristate(checked),
        });
    }
    for (attribute, property) in [
        ("aria-expanded", "expanded"),
        ("aria-selected", "selected"),
        ("aria-required", "required"),
        ("aria-readonly", "readonly"),
        ("aria-multiselectable", "multiselectable"),
        ("aria-busy", "busy"),
        ("aria-modal", "modal"),
        ("aria-atomic", "atomic"),
        ("aria-multiline", "multiline"),
    ] {
        if let Some(value) = node.get_attribute(attribute) {
            properties.push(AccessibilityProperty {
                name: property.to_string(),
                value: AccessibilityValue::Boolean(value.eq_ignore_ascii_case("true")),
            });
        }
    }
    if let Some(value) = node.get_attribute("aria-pressed") {
        properties.push(AccessibilityProperty {
            name: "pressed".to_string(),
            value: AccessibilityValue::Tristate(value.to_ascii_lowercase()),
        });
    }
    if node.get_attribute("required").is_some() && !has_property(&properties, "required") {
        push_boolean(&mut properties, "required", true);
    }
    if node.get_attribute("readonly").is_some() && !has_property(&properties, "readonly") {
        push_boolean(&mut properties, "readonly", true);
    }
    if tag == "option" && !has_property(&properties, "selected") {
        push_boolean(
            &mut properties,
            "selected",
            snapshot_state
                .selected_option_identities
                .contains(&node.identity()),
        );
    }
    let details = if tag == "details" {
        Some(node.clone())
    } else if tag == "summary" {
        node.parent_node()
            .filter(|parent| parent.tag_name().as_deref() == Some("details"))
    } else {
        None
    };
    if let Some(details) = details
        && !has_property(&properties, "expanded")
    {
        push_boolean(
            &mut properties,
            "expanded",
            snapshot_state
                .open_details_identities
                .contains(&details.identity()),
        );
    }
    if tag == "select"
        && node.get_attribute("multiple").is_some()
        && !has_property(&properties, "multiselectable")
    {
        push_boolean(&mut properties, "multiselectable", true);
    }
    if role == "heading" {
        let level = node
            .get_attribute("aria-level")
            .and_then(|level| level.parse::<i64>().ok())
            .or_else(|| tag.strip_prefix('h').and_then(|level| level.parse::<i64>().ok()));
        if let Some(level) = level {
            properties.push(AccessibilityProperty {
                name: "level".to_string(),
                value: AccessibilityValue::Integer(level),
            });
        }
    }
    for (attribute, property) in [("aria-valuemin", "valuemin"), ("aria-valuemax", "valuemax")] {
        if let Some(value) = node
            .get_attribute(attribute)
            .and_then(|value| value.parse::<f64>().ok())
        {
            properties.push(AccessibilityProperty {
                name: property.to_string(),
                value: AccessibilityValue::Number(value),
            });
        }
    }
    if let Some(value) = node.get_attribute("aria-valuetext") {
        properties.push(AccessibilityProperty {
            name: "valuetext".to_string(),
            value: AccessibilityValue::String(value),
        });
    }
    for (attribute, property) in [
        ("aria-live", "live"),
        ("aria-haspopup", "hasPopup"),
        ("aria-invalid", "invalid"),
        ("aria-autocomplete", "autocomplete"),
        ("aria-orientation", "orientation"),
    ] {
        if let Some(value) = node.get_attribute(attribute) {
            properties.push(AccessibilityProperty {
                name: property.to_string(),
                value: AccessibilityValue::Token(value),
            });
        }
    }
    if let Some(value) = node.get_attribute("aria-relevant") {
        properties.push(AccessibilityProperty {
            name: "relevant".to_string(),
            value: AccessibilityValue::TokenList(normalize_whitespace(&value)),
        });
    }
    for (attribute, property) in [
        ("aria-keyshortcuts", "keyshortcuts"),
        ("aria-roledescription", "roledescription"),
    ] {
        if let Some(value) = node.get_attribute(attribute) {
            properties.push(AccessibilityProperty {
                name: property.to_string(),
                value: AccessibilityValue::String(value),
            });
        }
    }
    properties
}

fn push_boolean(properties: &mut Vec<AccessibilityProperty>, name: &str, value: bool) {
    properties.push(AccessibilityProperty {
        name: name.to_string(),
        value: AccessibilityValue::Boolean(value),
    });
}

fn has_property(properties: &[AccessibilityProperty], name: &str) -> bool {
    properties.iter().any(|property| property.name == name)
}

fn aria_boolean(value: Option<String>) -> Option<bool> {
    value.map(|value| value.eq_ignore_ascii_case("true"))
}

fn is_focusable(node: &NodeHandle) -> bool {
    if is_actually_disabled(node) {
        return false;
    }
    if node.get_attribute("tabindex").is_some() {
        return true;
    }
    if node
        .get_attribute("contenteditable")
        .is_some_and(|value| value.is_empty() || value.eq_ignore_ascii_case("true"))
    {
        return true;
    }
    match node.tag_name().as_deref() {
        Some("a" | "area") => node.get_attribute("href").is_some(),
        Some("input") => input_type(node) != "hidden",
        Some("button" | "select" | "textarea" | "iframe" | "embed") => true,
        Some("audio" | "video") => node.get_attribute("controls").is_some(),
        Some("summary") => node
            .parent_node()
            .is_some_and(|parent| parent.tag_name().as_deref() == Some("details")),
        _ => false,
    }
}

fn has_global_aria_attribute_for_role(node: &NodeHandle, role: &str) -> bool {
    const GLOBAL_ARIA_ATTRIBUTES: &[&str] = &[
        "aria-atomic",
        "aria-busy",
        "aria-controls",
        "aria-current",
        "aria-describedby",
        "aria-details",
        "aria-disabled",
        "aria-dropeffect",
        "aria-errormessage",
        "aria-flowto",
        "aria-grabbed",
        "aria-haspopup",
        "aria-hidden",
        "aria-invalid",
        "aria-keyshortcuts",
        "aria-label",
        "aria-labelledby",
        "aria-live",
        "aria-owns",
        "aria-relevant",
        "aria-roledescription",
    ];
    node.attributes().is_some_and(|attributes| {
        attributes.keys().any(|name| {
            GLOBAL_ARIA_ATTRIBUTES.contains(&name.as_str())
                && (!role_prohibits_name(role)
                    || !matches!(name.as_str(), "aria-label" | "aria-labelledby"))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::TreeBuilder as HtmlTreeBuilder;

    fn tree(html: &str) -> AccessibilityTree {
        let document = HtmlTreeBuilder::parse(html).document();
        AccessibilityTree::build(
            &document,
            "Example",
            1,
            None,
            &AccessibilitySnapshotState::default(),
            |node| {
                let style = node
                    .get_attribute("style")
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .replace(' ', "");
                if style
                    .split(';')
                    .any(|declaration| declaration.starts_with("display:none"))
                {
                    AccessibilityRenderState::NotRendered
                } else if style
                    .split(';')
                    .any(|declaration| declaration.starts_with("visibility:hidden"))
                {
                    AccessibilityRenderState::NotVisible
                } else {
                    AccessibilityRenderState::Rendered
                }
            },
        )
    }

    #[test]
    fn exposes_native_roles_names_descriptions_and_control_state() {
        let tree = tree(
            "<html><body><label for='agree'>Accept terms</label>\
             <input id='agree' type='checkbox' checked aria-describedby='help'>\
             <span id='help'>Required to continue</span>\
             <button aria-pressed='mixed'>Save <strong>now</strong></button></body></html>",
        );
        let nodes = tree.nodes_preorder();
        let checkbox = nodes.iter().find(|node| node.role == "checkbox").unwrap();
        assert_eq!(checkbox.name, "Accept terms");
        assert_eq!(checkbox.description, "Required to continue");
        assert!(checkbox.properties.iter().any(|property| {
            property.name == "checked"
                && property.value == AccessibilityValue::Tristate("true".to_string())
        }));
        let button = nodes.iter().find(|node| node.role == "button").unwrap();
        assert_eq!(button.name, "Save now");
        assert!(button.properties.iter().any(|property| {
            property.name == "pressed"
                && property.value == AccessibilityValue::Tristate("mixed".to_string())
        }));
    }

    #[test]
    fn heading_levels_require_integer_values() {
        let tree = tree(
            "<html><body><h2 id='implicit'>Implicit</h2>\
             <div id='explicit' role='heading' aria-level='3'>Explicit</div>\
             <div id='fractional' role='heading' aria-level='2.7'>Invalid</div>\
             </body></html>",
        );
        let nodes = tree.nodes_preorder();
        let by_id = |id: &str| {
            nodes
                .iter()
                .copied()
                .find(|node| node.dom_node.get_attribute("id").as_deref() == Some(id))
                .unwrap()
        };
        let has_level = |node: &AccessibilityNode, level| {
            node.properties.iter().any(|property| {
                property.name == "level"
                    && property.value == AccessibilityValue::Integer(level)
            })
        };

        assert!(has_level(by_id("implicit"), 2));
        assert!(has_level(by_id("explicit"), 3));
        assert!(
            by_id("fractional")
                .properties
                .iter()
                .all(|property| property.name != "level")
        );
    }

    #[test]
    fn cyclic_label_references_terminate_and_relationships_keep_hidden_targets() {
        let tree = tree(
            "<html><body><span id='first' aria-labelledby='second'>First</span>\
             <span id='second' aria-labelledby='first' hidden>Second</span>\
             <button id='action' aria-labelledby='first' aria-describedby='second'>Go</button>\
             </body></html>",
        );
        let action = tree
            .nodes_preorder()
            .into_iter()
            .find(|node| node.dom_node.get_attribute("id").as_deref() == Some("action"))
            .unwrap();

        assert_eq!(action.name, "First");
        assert_eq!(action.description, "Second");
        assert!(action.properties.iter().any(|property| {
            property.name == "labelledby"
                && matches!(
                    &property.value,
                    AccessibilityValue::IdRefList {
                        value,
                        related_nodes,
                    } if value == "first" && related_nodes.len() == 1
                )
        }));
        assert!(action.properties.iter().any(|property| {
            property.name == "describedby"
                && matches!(
                    &property.value,
                    AccessibilityValue::IdRefList {
                        value,
                        related_nodes,
                    } if value == "second"
                        && related_nodes.len() == 1
                        && related_nodes[0].text == "Second"
                )
        }));
    }

    #[test]
    fn name_from_content_honors_hidden_named_and_embedded_descendants() {
        let tree = tree(
            "<html><body><button id='content'>Save <span hidden>secret</span>\
             <span aria-label='Delete'>×</span><span aria-label=''>fallback</span>\
             <span style='visibility:hidden'>hidden <span style='visibility:visible'>Visible</span></span></button>\
             <label id='volume'>Volume <span hidden>secret</span>\
             <input type='range' value='5' aria-label='Ignored'></label>\
             </body></html>",
        );
        let nodes = tree.nodes_preorder();
        let content = nodes
            .iter()
            .find(|node| node.dom_node.get_attribute("id").as_deref() == Some("content"))
            .unwrap();
        let volume = nodes
            .iter()
            .find(|node| node.dom_node.get_attribute("id").as_deref() == Some("volume"))
            .unwrap();

        assert_eq!(content.name, "Save × fallback Visible");
        assert_eq!(volume.name, "Volume 5");
    }

    #[test]
    fn labelledby_references_use_local_content_and_hiddenness() {
        let tree = tree(
            "<html><body><span id='nested'>Nested name</span>\
             <span id='visible-label' aria-labelledby='nested'>Visible name\
             <span hidden>hidden child</span></span>\
             <button id='visible' aria-labelledby='visible-label'>Fallback</button>\
             <div hidden><span id='hidden-label'>Hidden name\
             <span hidden>hidden descendant</span></span></div>\
             <button id='hidden' aria-labelledby='hidden-label'>Fallback</button>\
             </body></html>",
        );
        let nodes = tree.nodes_preorder();
        let by_id = |id: &str| {
            nodes
                .iter()
                .copied()
                .find(|node| node.dom_node.get_attribute("id").as_deref() == Some(id))
                .unwrap()
        };

        assert_eq!(by_id("visible").name, "Visible name");
        assert_eq!(by_id("hidden").name, "Hidden name hidden descendant");
    }

    #[test]
    fn name_prohibited_roles_ignore_author_names() {
        let tree = tree(
            "<html><body><span id='source'>Referenced name</span>\
             <div id='generic' aria-label='Generic label' aria-labelledby='source'>Text</div>\
             <p id='paragraph' aria-label='Paragraph label'>Text</p>\
             <code id='code' aria-labelledby='source'>Text</code></body></html>",
        );
        let nodes = tree.nodes_preorder();
        let by_id = |id: &str| {
            nodes
                .iter()
                .copied()
                .find(|node| node.dom_node.get_attribute("id").as_deref() == Some(id))
                .unwrap()
        };

        assert!(by_id("generic").ignored);
        assert_eq!(by_id("generic").name, "");
        assert_eq!(by_id("paragraph").name, "");
        assert_eq!(by_id("code").name, "");
    }

    #[test]
    fn empty_image_alt_is_ignored_unless_aria_exposes_the_image() {
        let tree = tree(
            "<html><body><img id='empty' alt=''><img id='named' alt='' aria-label='Portrait'></body></html>",
        );
        let nodes = tree.nodes_preorder();
        let by_id = |id: &str| {
            nodes
                .iter()
                .copied()
                .find(|node| node.dom_node.get_attribute("id").as_deref() == Some(id))
                .unwrap()
        };

        assert!(by_id("empty").ignored);
        assert_eq!(by_id("empty").ignored_reasons, ["emptyAlt"]);
        assert!(!by_id("named").ignored);
        assert_eq!(by_id("named").name, "Portrait");
    }

    #[test]
    fn visibility_hidden_text_is_pruned_but_visible_element_override_survives() {
        let document = HtmlTreeBuilder::parse(
            "<html><body><div id='hidden' style='visibility:hidden'>direct\
             <span id='shown' style='visibility:visible'>shown</span></div></body></html>",
        )
        .document();
        let render_state = |node: &NodeHandle| {
            let style = node
                .get_attribute("style")
                .unwrap_or_default()
                .replace(' ', "")
                .to_ascii_lowercase();
            if style.contains("visibility:hidden") {
                AccessibilityRenderState::NotVisible
            } else {
                AccessibilityRenderState::Rendered
            }
        };
        let tree = AccessibilityTree::build(
            &document,
            "Example",
            1,
            None,
            &AccessibilitySnapshotState::default(),
            render_state,
        );
        let hidden = document.query_selector("#hidden").unwrap();
        let shown = document.query_selector("#shown").unwrap();
        let direct_text = hidden
            .child_nodes()
            .into_iter()
            .find(|node| node.node_type() == NodeType::Text)
            .unwrap();
        let shown_text = shown.child_nodes()[0].clone();

        assert!(tree.find_by_dom_identity(direct_text.identity()).is_none());
        assert!(tree.find_by_dom_identity(shown_text.identity()).is_some());

        let direct = AccessibilityTree::build_inspected_node(
            &document,
            &direct_text,
            1,
            None,
            &AccessibilitySnapshotState::default(),
            render_state,
        )
        .unwrap();
        let shown = AccessibilityTree::build_inspected_node(
            &document,
            &shown_text,
            1,
            None,
            &AccessibilitySnapshotState::default(),
            render_state,
        )
        .unwrap();
        assert!(direct.ignored);
        assert_eq!(direct.ignored_reasons, ["notVisible"]);
        assert!(!shown.ignored);
    }

    #[test]
    fn aria_owns_reparents_same_scope_nodes_and_rejects_cycles() {
        let document = HtmlTreeBuilder::parse(
            "<html><body><section id='owner' aria-owns='target'></section>\
             <div><button id='target'>Owned</button></div></body></html>",
        )
        .document();
        let state = AccessibilitySnapshotState::default();
        let owned_tree = AccessibilityTree::build(&document, "Example", 1, None, &state, |_| {
            AccessibilityRenderState::Rendered
        });
        let owner = document.query_selector("#owner").unwrap();
        let target = document.query_selector("#target").unwrap();
        let path = owned_tree.path_to_dom_identity(target.identity()).unwrap();
        assert_eq!(path[path.len() - 2].dom_node, owner);

        let cyclic_document = HtmlTreeBuilder::parse(
            "<html><body><div id='ancestor'><section id='descendant' aria-owns='ancestor'></section></div></body></html>",
        )
        .document();
        let cyclic_tree =
            AccessibilityTree::build(&cyclic_document, "Example", 1, None, &state, |_| {
                AccessibilityRenderState::Rendered
            });
        let ancestor = cyclic_document.query_selector("#ancestor").unwrap();
        let descendant = cyclic_document.query_selector("#descendant").unwrap();
        let descendant_path = cyclic_tree
            .path_to_dom_identity(descendant.identity())
            .unwrap();
        assert!(descendant_path.iter().any(|node| node.dom_node == ancestor));

        let hidden_owner_tree = tree(
            "<html><body><div id='hidden-owner' style='display:none' aria-owns='visible-target'></div>\
             <textarea id='visible-target'>Value</textarea></body></html>",
        );
        let hidden_owner = hidden_owner_tree
            .nodes_preorder()
            .into_iter()
            .find(|node| node.dom_node.get_attribute("id").as_deref() == Some("hidden-owner"))
            .unwrap();
        assert!(hidden_owner.ignored);
        assert_eq!(hidden_owner.ignored_reasons, ["notRendered"]);
        assert!(hidden_owner.children.iter().any(|child| {
            child.dom_node.get_attribute("id").as_deref() == Some("visible-target")
        }));

        let leaf_tree = tree(
            "<html><body><input id='leaf' aria-owns='leaf-target'>\
             <div id='leaf-target'>Target</div></body></html>",
        );
        let leaf = leaf_tree
            .nodes_preorder()
            .into_iter()
            .find(|node| node.dom_node.get_attribute("id").as_deref() == Some("leaf"))
            .unwrap();
        assert!(
            leaf.children.iter().all(|child| {
                child.dom_node.get_attribute("id").as_deref() != Some("leaf-target")
            })
        );
    }

    #[test]
    fn aria_labelledby_wins_and_hidden_subtrees_are_pruned() {
        let tree = tree(
            "<html><body><h1 id='first'>Account</h1><span id='second' hidden>settings</span>\
             <section role='region' aria-labelledby='first second'></section>\
             <nav aria-hidden='true'><a href='/private'>Private</a></nav>\
             <button style='display:none'>Invisible</button></body></html>",
        );
        let nodes = tree.nodes_preorder();
        let region = nodes.iter().find(|node| node.role == "region").unwrap();
        assert_eq!(region.name, "Account settings");
        let hidden_nav = nodes.iter().find(|node| node.role == "navigation").unwrap();
        assert!(hidden_nav.ignored);
        assert!(hidden_nav.children.is_empty());
        assert!(!nodes.iter().any(|node| node.name == "Private"));
    }

    #[test]
    fn presentational_role_conflicts_preserve_focusable_and_global_aria_nodes() {
        let tree = tree(
            "<html><body><button id='native' role='none'>Native button</button>\
             <div id='focusable' role='presentation' tabindex='0'>Focusable generic</div>\
             <div id='named' role='none' aria-label='Named generic'></div>\
             <div id='global' role='none' aria-live='polite'></div>\
             <span id='plain' role='presentation'>Plain</span></body></html>",
        );
        let nodes = tree.nodes_preorder();
        let by_id = |id: &str| {
            nodes
                .iter()
                .copied()
                .find(|node| node.dom_node.get_attribute("id").as_deref() == Some(id))
                .unwrap()
        };

        assert_eq!(by_id("native").role, "button");
        assert!(!by_id("native").ignored);
        assert_eq!(by_id("focusable").role, "generic");
        assert!(!by_id("focusable").ignored);
        assert_eq!(by_id("named").role, "none");
        assert_eq!(by_id("named").name, "");
        assert!(by_id("named").ignored);
        assert_eq!(by_id("global").role, "generic");
        assert!(!by_id("global").ignored);
        assert!(by_id("plain").ignored);
        assert!(
            by_id("plain")
                .ignored_reasons
                .contains(&"presentationalRole".to_string())
        );
    }

    #[test]
    fn composed_shadow_tree_replaces_unassigned_light_dom() {
        let document = HtmlTreeBuilder::parse(
            "<html><body><div id='host'><button>Light</button></div></body></html>",
        )
        .document();
        let host = document.query_selector("#host").unwrap();
        let root = host
            .attach_shadow(crate::dom::ShadowRootMode::Open)
            .unwrap();
        let shadow_button = NodeHandle::element("button");
        shadow_button.append_child(NodeHandle::text("Shadow"));
        root.append_child(shadow_button);

        let tree = AccessibilityTree::build(
            &document,
            "Example",
            1,
            None,
            &AccessibilitySnapshotState::default(),
            |_| AccessibilityRenderState::Rendered,
        );
        let names = tree
            .nodes_preorder()
            .iter()
            .map(|node| node.name.as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"Shadow"));
        assert!(!names.contains(&"Light"));
    }

    #[test]
    fn id_references_are_scoped_to_their_shadow_tree() {
        let document = HtmlTreeBuilder::parse(
            "<html><body><span id='shared'>Light label</span><div id='host'></div></body></html>",
        )
        .document();
        let host = document.query_selector("#host").unwrap();
        let root = host
            .attach_shadow(crate::dom::ShadowRootMode::Closed)
            .unwrap();
        let shadow_label = NodeHandle::element("span");
        shadow_label.set_attribute("id", "shared");
        shadow_label.append_child(NodeHandle::text("Shadow label"));
        let button = NodeHandle::element("button");
        button.set_attribute("id", "shadow-button");
        button.set_attribute("aria-labelledby", "shared");
        root.append_child(shadow_label.clone());
        root.append_child(button.clone());

        let tree = AccessibilityTree::build(
            &document,
            "Example",
            1,
            None,
            &AccessibilitySnapshotState::default(),
            |_| AccessibilityRenderState::Rendered,
        );
        let button = tree.find_by_dom_identity(button.identity()).unwrap();
        assert_eq!(button.name, "Shadow label");
        let labelledby = button
            .properties
            .iter()
            .find(|property| property.name == "labelledby")
            .unwrap();
        assert!(matches!(
            &labelledby.value,
            AccessibilityValue::IdRefList { related_nodes, .. }
                if related_nodes[0].dom_node == shadow_label
        ));
    }
}
