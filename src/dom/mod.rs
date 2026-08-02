//! DOM primitives.
//!
//! The DOM layer uses reference-counted node handles so later parser and style
//! phases can share and mutate the same tree.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Monotonic source of per-node identities. A fresh value is minted for every
/// node and never reused, so [`NodeHandle::identity`] cannot alias a released
/// node's identity. (A pointer-based identity would be recycled when the
/// allocator reuses a freed node's address, which caused JS wrapper caches keyed
/// by identity to resolve a newly created node to a stale wrapper — see the
/// iframe-reload lifetime hazard in issue 049.)
static NEXT_NODE_ID: AtomicUsize = AtomicUsize::new(1);

thread_local! {
    /// Number of elements on this thread that currently hold a non-zero scroll
    /// offset. Scrolling is rare, so paint and the layout-metrics bindings use
    /// this to skip their scroll passes entirely on documents that never
    /// scrolled. A released element does not decrement the count, which only
    /// costs an avoidable pass.
    static SCROLLED_ELEMENTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Whether any element on this thread holds a non-zero scroll offset.
pub(crate) fn any_element_scrolled() -> bool {
    SCROLLED_ELEMENTS.with(|count| count.get() > 0)
}

/// Returns whether an HTML form control is actually disabled.
///
/// Besides a control's own `disabled` attribute, a disabled `fieldset`
/// disables descendant controls except those inside its first direct `legend`
/// child. The exception is evaluated independently for every ancestor
/// fieldset, which also covers nested fieldsets.
pub(crate) fn is_actually_disabled(node: &NodeHandle) -> bool {
    const DISABLEABLE_TAGS: &[&str] = &[
        "button", "input", "select", "textarea", "option", "optgroup", "fieldset",
    ];

    let Some(tag) = node.tag_name() else {
        return false;
    };
    if !DISABLEABLE_TAGS.contains(&tag.as_str()) {
        return false;
    }
    if node.get_attribute("disabled").is_some() {
        return true;
    }

    let mut ancestor = node.parent_node();
    while let Some(current) = ancestor {
        if current.tag_name().as_deref() == Some("fieldset")
            && current.get_attribute("disabled").is_some()
        {
            let first_legend = current
                .child_nodes()
                .into_iter()
                .find(|child| child.tag_name().as_deref() == Some("legend"));
            let inside_first_legend = first_legend.is_some_and(|legend| {
                let mut descendant = Some(node.clone());
                while let Some(candidate) = descendant {
                    if candidate == legend {
                        return true;
                    }
                    if candidate == current {
                        break;
                    }
                    descendant = candidate.parent_node();
                }
                false
            });
            if !inside_first_legend {
                return true;
            }
        }
        ancestor = current.parent_node();
    }
    false
}

/// A handle to a DOM node.
#[derive(Clone, Debug)]
pub struct NodeHandle(Rc<RefCell<NodeInner>>);

/// A non-owning handle used by caches that must not extend a DOM node's lifetime.
#[derive(Clone)]
pub(crate) struct WeakNodeHandle(Weak<RefCell<NodeInner>>);

impl WeakNodeHandle {
    pub(crate) fn is_alive(&self) -> bool {
        self.0.strong_count() != 0
    }
}

impl PartialEq for NodeHandle {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for NodeHandle {}

#[derive(Debug)]
struct NodeInner {
    /// Stable, never-reused identity for this node (see [`NEXT_NODE_ID`]).
    id: usize,
    parent: Option<Weak<RefCell<NodeInner>>>,
    children: Vec<NodeHandle>,
    data: NodeData,
}

#[derive(Debug, Clone)]
enum NodeData {
    Document(Document),
    DocumentFragment,
    ShadowRoot(ShadowRoot),
    Element(Element),
    Text(Text),
    Comment(Comment),
    ProcessingInstruction(ProcessingInstruction),
    DocumentType(DocumentType),
}

/// DOM node kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeType {
    Document,
    DocumentFragment,
    Element,
    Text,
    Comment,
    ProcessingInstruction,
    DocumentType,
}

/// Shadow tree visibility requested through `Element.attachShadow()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadowRootMode {
    Open,
    Closed,
}

/// Basic DOM node operations.
pub trait Node {
    /// Returns the DOM node type.
    fn node_type(&self) -> NodeType;

    /// Returns the DOM node name.
    fn node_name(&self) -> String;

    /// Returns the parent node, if any.
    fn parent_node(&self) -> Option<NodeHandle>;

    /// Returns the current child nodes.
    fn child_nodes(&self) -> Vec<NodeHandle>;
}

/// A DOM document node.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Document;

/// The host relationship and visibility of a shadow tree root.
#[derive(Debug, Clone)]
struct ShadowRoot {
    host: Weak<RefCell<NodeInner>>,
    mode: ShadowRootMode,
}

/// A DOM element node.
///
/// `Eq` is deliberately not derived: [`Element::scroll_offset`] is a float pair,
/// and element identity comes from the node handle rather than field equality.
#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    tag_name: String,
    namespace_uri: Option<String>,
    prefix: Option<String>,
    local_name: String,
    html: bool,
    attributes: BTreeMap<String, String>,
    attribute_names: BTreeMap<String, AttributeName>,
    checked: bool,
    dirty_checkedness: bool,
    text_control_state: Option<TextControlState>,
    /// Scroll offset of this element's scrolling box in CSS pixels, as set
    /// through `scrollTop` / `scrollLeft` and friends.
    ///
    /// It is stored unclamped: consumers clamp it against the current scrollable
    /// extent, so an offset survives a temporary `display: none` or
    /// `overflow: visible` and comes back when the box does, matching browsers.
    /// Detaching the element drops it, because that destroys the box.
    scroll_offset: (f32, f32),
    /// The inert template contents owner for HTML `<template>` elements.
    ///
    /// Template contents are not children of the element itself. Keeping the
    /// fragment in the native DOM model makes parser-created templates inert
    /// before JavaScript wrappers or layout ever inspect the document tree.
    template_content: Option<NodeHandle>,
    /// The element's shadow tree. It is deliberately not part of `children`:
    /// light DOM traversal and document selectors must not cross this boundary.
    shadow_root: Option<NodeHandle>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AttributeName {
    namespace_uri: Option<String>,
    local_name: String,
}

impl Element {
    /// Creates a new element payload.
    pub fn new(tag_name: impl Into<String>) -> Self {
        let tag_name = tag_name.into().to_ascii_lowercase();
        let template_content = (tag_name == "template").then(NodeHandle::document_fragment);
        Self {
            tag_name,
            namespace_uri: None,
            prefix: None,
            local_name: String::new(),
            html: true,
            attributes: BTreeMap::new(),
            attribute_names: BTreeMap::new(),
            checked: false,
            dirty_checkedness: false,
            text_control_state: None,
            scroll_offset: (0.0, 0.0),
            template_content,
            shadow_root: None,
        }
    }

    pub fn new_xml(
        qualified_name: impl Into<String>,
        namespace_uri: Option<String>,
    ) -> Self {
        let tag_name = qualified_name.into();
        let (prefix, local_name) = tag_name
            .split_once(':')
            .map(|(prefix, local)| (Some(prefix.to_string()), local.to_string()))
            .unwrap_or_else(|| (None, tag_name.clone()));
        Self {
            tag_name,
            namespace_uri,
            prefix,
            local_name,
            html: false,
            attributes: BTreeMap::new(),
            attribute_names: BTreeMap::new(),
            checked: false,
            dirty_checkedness: false,
            text_control_state: None,
            scroll_offset: (0.0, 0.0),
            template_content: None,
            shadow_root: None,
        }
    }

    pub fn new_html_ns(
        qualified_name: impl Into<String>,
        namespace_uri: impl Into<String>,
    ) -> Self {
        // `createElementNS()` preserves its qualified name even for the HTML namespace.
        let mut element = Self::new_xml(qualified_name, Some(namespace_uri.into()));
        element.html = true;
        element.template_content = element.local_name.eq_ignore_ascii_case("template")
            .then(NodeHandle::document_fragment);
        element
    }

    /// Returns the normalized tag name.
    pub fn tag_name(&self) -> &str {
        &self.tag_name
    }

    pub fn namespace_uri(&self) -> Option<&str> { self.namespace_uri.as_deref() }
    pub fn prefix(&self) -> Option<&str> { self.prefix.as_deref() }
    pub fn local_name(&self) -> &str {
        if self.local_name.is_empty() { &self.tag_name } else { &self.local_name }
    }
    pub fn is_html(&self) -> bool { self.html }

    /// Returns the element attributes.
    pub fn attributes(&self) -> &BTreeMap<String, String> {
        &self.attributes
    }
}

/// Live value and selection state for a text form control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextControlState {
    pub value: String,
    pub selection_start: usize,
    pub selection_end: usize,
    pub focused: bool,
}

/// A DOM text node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Text {
    data: String,
}

impl Text {
    /// Creates a new text payload.
    pub fn new(data: impl Into<String>) -> Self {
        Self { data: data.into() }
    }

    /// Returns the text contents.
    pub fn data(&self) -> &str {
        &self.data
    }
}

/// A DOM comment node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    data: String,
}

impl Comment {
    /// Creates a new comment payload.
    pub fn new(data: impl Into<String>) -> Self {
        Self { data: data.into() }
    }

    /// Returns the comment contents.
    pub fn data(&self) -> &str {
        &self.data
    }
}

/// A DOM processing instruction node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessingInstruction {
    target: String,
    data: String,
}

impl ProcessingInstruction {
    /// Creates a processing instruction payload.
    pub fn new(target: impl Into<String>, data: impl Into<String>) -> Self {
        Self {
            target: target.into(),
            data: data.into(),
        }
    }
}

/// A DOM document type node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentType {
    name: String,
    public_id: Option<String>,
    system_id: Option<String>,
}

impl DocumentType {
    /// Creates a new document type payload.
    pub fn new(
        name: impl Into<String>,
        public_id: impl Into<String>,
        system_id: impl Into<String>,
    ) -> Self {
        let public_id = public_id.into();
        let system_id = system_id.into();
        Self {
            name: name.into(),
            public_id: (!public_id.is_empty()).then_some(public_id),
            system_id: (!system_id.is_empty()).then_some(system_id),
        }
    }

    /// Returns the doctype name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the public identifier, if any.
    pub fn public_id(&self) -> Option<&str> {
        self.public_id.as_deref()
    }

    /// Returns the system identifier, if any.
    pub fn system_id(&self) -> Option<&str> {
        self.system_id.as_deref()
    }
}

/// Errors returned by DOM tree operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomError {
    ReferenceChildNotFound,
    ChildNotFound,
    /// The operation would make a node an ancestor of itself (a cyclic tree).
    /// Corresponds to the DOM `HierarchyRequestError`.
    HierarchyRequest,
}

impl fmt::Display for DomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReferenceChildNotFound => write!(f, "reference child not found"),
            Self::ChildNotFound => write!(f, "child not found"),
            Self::HierarchyRequest => write!(f, "hierarchy request error"),
        }
    }
}

impl std::error::Error for DomError {}

impl NodeHandle {
    /// Creates a document node.
    pub fn document() -> Self {
        Self::new(NodeData::Document(Document))
    }

    /// Creates a document fragment node.
    pub fn document_fragment() -> Self {
        Self::new(NodeData::DocumentFragment)
    }

    /// Creates an element node.
    pub fn element(tag_name: impl Into<String>) -> Self {
        Self::new(NodeData::Element(Element::new(tag_name)))
    }

    /// Creates an HTML element with explicit namespace metadata.
    pub fn html_element_ns(
        qualified_name: impl Into<String>,
        namespace_uri: impl Into<String>,
    ) -> Self {
        Self::new(NodeData::Element(Element::new_html_ns(
            qualified_name,
            namespace_uri,
        )))
    }

    /// Creates an XML element, preserving its qualified name and namespace.
    pub fn xml_element(tag_name: impl Into<String>, namespace_uri: Option<String>) -> Self {
        Self::new(NodeData::Element(Element::new_xml(tag_name, namespace_uri)))
    }

    /// Creates a text node.
    pub fn text(data: impl Into<String>) -> Self {
        Self::new(NodeData::Text(Text::new(data)))
    }

    /// Creates a comment node.
    pub fn comment(data: impl Into<String>) -> Self {
        Self::new(NodeData::Comment(Comment::new(data)))
    }

    /// Creates a processing instruction node.
    pub fn processing_instruction(target: impl Into<String>, data: impl Into<String>) -> Self {
        Self::new(NodeData::ProcessingInstruction(ProcessingInstruction::new(target, data)))
    }

    /// Creates a document type node.
    pub fn document_type(
        name: impl Into<String>,
        public_id: impl Into<String>,
        system_id: impl Into<String>,
    ) -> Self {
        Self::new(NodeData::DocumentType(DocumentType::new(
            name, public_id, system_id,
        )))
    }

    fn new(data: NodeData) -> Self {
        Self(Rc::new(RefCell::new(NodeInner {
            id: NEXT_NODE_ID.fetch_add(1, Ordering::Relaxed),
            parent: None,
            children: Vec::new(),
            data,
        })))
    }

    /// Returns a stable identity for this node handle.
    ///
    /// The identity is minted once per node and never reused, so it stays valid
    /// as an id-keyed map key even after the node is released: unlike a
    /// pointer-derived identity, a later node can never reuse it (see
    /// [`NEXT_NODE_ID`]). Clones of a [`NodeHandle`] share one node and thus one
    /// identity.
    pub(crate) fn identity(&self) -> usize {
        self.0.borrow().id
    }

    pub(crate) fn downgrade(&self) -> WeakNodeHandle {
        WeakNodeHandle(Rc::downgrade(&self.0))
    }

    /// Returns the inert contents fragment owned by an HTML `<template>`.
    pub fn template_content(&self) -> Option<NodeHandle> {
        match &self.0.borrow().data {
            NodeData::Element(element) => element.template_content.clone(),
            _ => None,
        }
    }

    /// Creates and attaches a shadow root, returning `None` when this is not an
    /// element or it already owns one.
    pub fn attach_shadow(&self, mode: ShadowRootMode) -> Option<NodeHandle> {
        let mut inner = self.0.borrow_mut();
        let NodeData::Element(element) = &mut inner.data else {
            return None;
        };
        if element.shadow_root.is_some() {
            return None;
        }
        let root = Self::new(NodeData::ShadowRoot(ShadowRoot {
            host: Rc::downgrade(&self.0),
            mode,
        }));
        element.shadow_root = Some(root.clone());
        Some(root)
    }

    /// Returns the shadow root attached to an element, including closed roots.
    pub fn shadow_root(&self) -> Option<NodeHandle> {
        match &self.0.borrow().data {
            NodeData::Element(element) => element.shadow_root.clone(),
            _ => None,
        }
    }

    /// Returns the host of a shadow root.
    pub fn shadow_host(&self) -> Option<NodeHandle> {
        match &self.0.borrow().data {
            NodeData::ShadowRoot(root) => root.host.upgrade().map(NodeHandle),
            _ => None,
        }
    }

    /// Returns the visibility mode of a shadow root.
    pub fn shadow_root_mode(&self) -> Option<ShadowRootMode> {
        match &self.0.borrow().data {
            NodeData::ShadowRoot(root) => Some(root.mode),
            _ => None,
        }
    }

    /// Returns the shadow root containing this node, if it is in a shadow tree.
    /// The shadow root itself is returned when called on the root.
    pub fn containing_shadow_root(&self) -> Option<NodeHandle> {
        let mut current = Some(self.clone());
        while let Some(node) = current {
            if matches!(&node.0.borrow().data, NodeData::ShadowRoot(_)) {
                return Some(node);
            }
            current = node.parent_node();
        }
        None
    }

    /// Returns the slot this light-tree child is assigned to.
    ///
    /// Assignment is derived from the current trees rather than cached. This
    /// keeps all native mutation paths (parser insertion, `innerHTML`, and DOM
    /// methods) consistent without requiring a second invalidation graph.
    pub fn assigned_slot(&self) -> Option<NodeHandle> {
        if !self.is_slottable() {
            return None;
        }
        let host = self.parent_node()?;
        let root = host.shadow_root()?;
        slot_assignments(&root)
            .into_iter()
            .find_map(|(slot, nodes)| nodes.iter().any(|node| node == self).then_some(slot))
    }

    /// Returns the directly assigned slottables for an HTML `<slot>` element.
    /// With `flatten`, nested slot elements are recursively replaced by their
    /// assigned nodes, or by their fallback children when unassigned.
    pub fn assigned_nodes(&self, flatten: bool) -> Vec<NodeHandle> {
        if !self.is_html_slot() {
            return Vec::new();
        }
        let Some(root) = self.containing_shadow_root() else {
            return Vec::new();
        };
        let assigned = slot_assignments(&root)
            .into_iter()
            .find_map(|(slot, nodes)| (&slot == self).then_some(nodes))
            .unwrap_or_default();
        if !flatten {
            return assigned;
        }

        let source = if assigned.is_empty() {
            self.child_nodes()
        } else {
            assigned
        };
        let mut flattened = Vec::new();
        flatten_slotables(source, &mut flattened);
        flattened
    }

    /// Returns children in the flat tree used for rendering.
    ///
    /// A shadow host contributes its shadow tree instead of its light children,
    /// and `<slot>` nodes are transparent, contributing assigned slottables or
    /// fallback content. DOM APIs continue to use [`Node::child_nodes`] and
    /// therefore preserve the light/shadow tree boundaries.
    pub fn layout_child_nodes(&self) -> Vec<NodeHandle> {
        let source = self
            .shadow_root()
            .map(|root| root.child_nodes())
            .unwrap_or_else(|| self.child_nodes());
        let mut children = Vec::new();
        for child in source {
            if child.is_html_slot() && child.containing_shadow_root().is_some() {
                let assigned = child.assigned_nodes(false);
                let slotables = if assigned.is_empty() {
                    child.child_nodes()
                } else {
                    assigned
                };
                flatten_slotables(slotables, &mut children);
            } else {
                children.push(child);
            }
        }
        children
    }

    fn is_html_slot(&self) -> bool {
        matches!(
            &self.0.borrow().data,
            NodeData::Element(element)
                if element.is_html() && element.local_name().eq_ignore_ascii_case("slot")
        )
    }

    fn is_slottable(&self) -> bool {
        matches!(self.node_type(), NodeType::Element | NodeType::Text)
    }

    /// Appends `child` to the node's children.
    ///
    /// Appending an inclusive ancestor of this node (or this node itself) would
    /// make the tree cyclic and hang every recursive traversal, so the request
    /// is silently rejected. The DOM spec raises a `HierarchyRequestError` in
    /// this case; here the operation is simply a no-op to keep the tree acyclic.
    pub fn append_child(&self, child: NodeHandle) {
        if child.is_inclusive_ancestor_of(self) {
            return;
        }
        detach_from_parent(&child);
        child.0.borrow_mut().parent = Some(Rc::downgrade(&self.0));
        self.0.borrow_mut().children.push(child);
    }

    /// Inserts `new_child` before `reference_child`.
    ///
    /// Returns [`DomError::HierarchyRequest`] if `new_child` is an inclusive
    /// ancestor of this node, which would otherwise create a cyclic tree.
    pub fn insert_before(
        &self,
        new_child: NodeHandle,
        reference_child: &NodeHandle,
    ) -> Result<(), DomError> {
        if new_child.is_inclusive_ancestor_of(self) {
            return Err(DomError::HierarchyRequest);
        }

        // The DOM pre-insert algorithm replaces a self-reference with the
        // node's next sibling. More generally, detach a moving node before
        // resolving the reference index: when the node was an earlier sibling,
        // its removal shifts that index left. Resolving the index first would
        // incorrectly place it after the reference child.
        let reference_after_move = {
            let parent = self.0.borrow();
            let reference_index = parent
                .children
                .iter()
                .position(|child| child == reference_child)
                .ok_or(DomError::ReferenceChildNotFound)?;
            if &new_child == reference_child {
                parent.children.get(reference_index + 1).cloned()
            } else {
                Some(reference_child.clone())
            }
        };

        detach_from_parent(&new_child);
        let index = if let Some(reference) = reference_after_move {
            self.0
                .borrow()
                .children
                .iter()
                .position(|child| child == &reference)
                .ok_or(DomError::ReferenceChildNotFound)?
        } else {
            self.0.borrow().children.len()
        };
        new_child.0.borrow_mut().parent = Some(Rc::downgrade(&self.0));
        self.0.borrow_mut().children.insert(index, new_child);
        Ok(())
    }

    /// Returns `true` if this node is `other` or one of its ancestors. Used to
    /// reject insertions that would make the tree cyclic.
    fn is_inclusive_ancestor_of(&self, other: &NodeHandle) -> bool {
        let mut current = Some(other.clone());
        while let Some(node) = current {
            if &node == self {
                return true;
            }
            current = node.parent_node();
        }
        false
    }

    /// Removes `child` from the node's children and returns it.
    pub fn remove_child(&self, child: &NodeHandle) -> Result<NodeHandle, DomError> {
        let index = self
            .0
            .borrow()
            .children
            .iter()
            .position(|candidate| candidate == child)
            .ok_or(DomError::ChildNotFound)?;

        let removed = self.0.borrow_mut().children.remove(index);
        removed.0.borrow_mut().parent = None;
        // Detaching destroys the subtree's boxes, and with them their scroll
        // offsets: re-inserting the node starts from the top of its content.
        // Reordering within one parent detaches first, so it resets too.
        if any_element_scrolled() {
            clear_scroll_offsets(&removed);
        }
        Ok(removed)
    }

    /// Returns the element tag name, if this is an element node.
    pub fn tag_name(&self) -> Option<String> {
        match &self.0.borrow().data {
            NodeData::Element(element) => Some(element.tag_name.clone()),
            _ => None,
        }
    }

    /// Returns a clone of the element attributes, if this is an element node.
    pub fn attributes(&self) -> Option<BTreeMap<String, String>> {
        match &self.0.borrow().data {
            NodeData::Element(element) => Some(element.attributes.clone()),
            _ => None,
        }
    }

    /// Returns qualified name, namespace, local name, and value for each
    /// element attribute. Prefixes remain in the qualified name but are not
    /// part of the equality key.
    pub fn attribute_records(&self) -> Option<Vec<(String, Option<String>, String, String)>> {
        match &self.0.borrow().data {
            NodeData::Element(element) => Some(element.attributes.iter().map(|(name, value)| {
                let metadata = element.attribute_names.get(name);
                (
                    name.clone(),
                    metadata.and_then(|entry| entry.namespace_uri.clone()),
                    metadata.map(|entry| entry.local_name.clone()).unwrap_or_else(|| name.clone()),
                    value.clone(),
                )
            }).collect()),
            _ => None,
        }
    }

    /// Returns a clone of one element attribute value, if it exists.
    ///
    /// The exact attribute name is checked first to preserve case-sensitive XML
    /// names, followed by an ASCII-lowercase lookup for HTML-style names.
    pub fn get_attribute(&self, name: &str) -> Option<String> {
        match &self.0.borrow().data {
            NodeData::Element(element) => element
                .attributes
                .get(name)
                .or_else(|| {
                    element
                        .html
                        .then(|| element.attributes.get(&name.to_ascii_lowercase()))
                        .flatten()
                })
                .cloned(),
            _ => None,
        }
    }

    /// Sets an attribute on an element node. No-op for other node kinds.
    pub fn set_attribute(&self, name: impl Into<String>, value: impl Into<String>) {
        if let NodeData::Element(element) = &mut self.0.borrow_mut().data {
            let name = name.into();
            let name = if element.html { name.to_ascii_lowercase() } else { name };
            if name == "checked" && !element.dirty_checkedness {
                element.checked = true;
            }
            element.attributes.insert(name.clone(), value.into());
            element
                .attribute_names
                .entry(name.clone())
                .or_insert_with(|| AttributeName {
                    namespace_uri: None,
                    local_name: name,
                });
        }
    }


    /// Sets an XML attribute without HTML ASCII case folding.
    pub fn set_xml_attribute(&self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into();
        self.set_xml_attribute_ns(name.clone(), None, name, value);
    }

    /// Sets an XML/namespaced attribute without HTML ASCII case folding.
    pub fn set_xml_attribute_ns(
        &self,
        qualified_name: impl Into<String>,
        namespace_uri: Option<String>,
        local_name: impl Into<String>,
        value: impl Into<String>,
    ) {
        if let NodeData::Element(element) = &mut self.0.borrow_mut().data {
            let qualified_name = qualified_name.into();
            element.attributes.insert(qualified_name.clone(), value.into());
            element.attribute_names.insert(qualified_name, AttributeName {
                namespace_uri,
                local_name: local_name.into(),
            });
        }
    }

    pub fn namespace_uri(&self) -> Option<String> {
        match &self.0.borrow().data {
            NodeData::Element(element) => element.namespace_uri().map(str::to_string),
            _ => None,
        }
    }

    /// Returns whether this node is an element created with HTML semantics.
    pub fn is_html_element(&self) -> bool {
        matches!(&self.0.borrow().data, NodeData::Element(element) if element.is_html())
    }

    pub fn prefix(&self) -> Option<String> {
        match &self.0.borrow().data {
            NodeData::Element(element) => element.prefix().map(str::to_string),
            _ => None,
        }
    }

    pub fn local_name(&self) -> Option<String> {
        match &self.0.borrow().data {
            NodeData::Element(element) => Some(element.local_name().to_string()),
            _ => None,
        }
    }

    /// Removes an attribute from an element node. No-op for other node kinds.
    pub fn remove_attribute(&self, name: &str) {
        if let NodeData::Element(element) = &mut self.0.borrow_mut().data {
            let name = if element.html { name.to_ascii_lowercase() } else { name.to_string() };
            element.attributes.remove(&name);
            element.attribute_names.remove(&name);
            if name == "checked" && !element.dirty_checkedness {
                element.checked = false;
            }
        }
    }

    /// Removes an exactly-qualified XML/namespaced attribute.
    pub fn remove_xml_attribute(&self, qualified_name: &str) {
        if let NodeData::Element(element) = &mut self.0.borrow_mut().data {
            element.attributes.remove(qualified_name);
            element.attribute_names.remove(qualified_name);
        }
    }

    /// Returns the live checkedness of an element.
    ///
    /// Checkedness is only meaningful for checkable inputs (`<input
    /// type="checkbox">` / `<input type="radio">`), matching `:checked` and
    /// HTML semantics: any other element reports `false` even if a stray
    /// `checked` attribute recorded internal state. The internal state is
    /// still kept in sync with the content attribute, so an input whose
    /// `type` later becomes checkable reports the attribute-derived
    /// checkedness, as the non-dirty default would.
    pub fn checked(&self) -> bool {
        if !self.is_checkable_input() {
            return false;
        }
        match &self.0.borrow().data {
            NodeData::Element(element) => element.checked,
            _ => false,
        }
    }

    /// Returns `true` for `<input>` elements whose `type` attribute makes
    /// them checkable (`checkbox` or `radio`, ASCII case-insensitive).
    pub fn is_checkable_input(&self) -> bool {
        match &self.0.borrow().data {
            NodeData::Element(element) => {
                element.tag_name == "input"
                    && element.attributes.get("type").is_some_and(|kind| {
                        matches!(kind.to_ascii_lowercase().as_str(), "checkbox" | "radio")
                    })
            }
            _ => false,
        }
    }

    /// Updates live checkedness and marks it dirty with respect to the content attribute.
    pub fn set_checked(&self, checked: bool) {
        if let NodeData::Element(element) = &mut self.0.borrow_mut().data {
            element.checked = checked;
            element.dirty_checkedness = true;
        }
    }

    /// Updates the live editing state used by layout and paint for text controls.
    pub(crate) fn set_text_control_state(
        &self,
        value: String,
        selection_start: usize,
        selection_end: usize,
        focused: bool,
    ) {
        if let NodeData::Element(element) = &mut self.0.borrow_mut().data {
            element.text_control_state = Some(TextControlState {
                value,
                selection_start: selection_start.min(selection_end),
                selection_end,
                focused,
            });
        }
    }

    /// Returns the live editing state for a text control, when JavaScript has initialized it.
    pub(crate) fn text_control_state(&self) -> Option<TextControlState> {
        match &self.0.borrow().data {
            NodeData::Element(element) => element.text_control_state.clone(),
            _ => None,
        }
    }

    /// Returns this element's stored scroll offset in CSS pixels, or
    /// `(0.0, 0.0)` for a node kind that has no scrolling box of its own.
    ///
    /// An element reports what was last stored on it whether or not it can
    /// currently scroll: the offset outlives a `display: none` or an
    /// `overflow: visible` and applies again once the box is back. Callers that
    /// need the offset in effect right now — clamped to the scrollable extent,
    /// and zero while the box cannot scroll — use
    /// [`crate::layout::LayoutBox::scroll_offset`] instead.
    pub(crate) fn scroll_offset(&self) -> (f32, f32) {
        match &self.0.borrow().data {
            NodeData::Element(element) => element.scroll_offset,
            _ => (0.0, 0.0),
        }
    }

    /// Stores this element's scroll offset in CSS pixels. No-op for other node
    /// kinds, which have no scrolling box.
    pub(crate) fn set_scroll_offset(&self, x: f32, y: f32) {
        let mut inner = self.0.borrow_mut();
        let NodeData::Element(element) = &mut inner.data else {
            return;
        };
        let was_scrolled = element.scroll_offset != (0.0, 0.0);
        element.scroll_offset = (x, y);
        let is_scrolled = element.scroll_offset != (0.0, 0.0);
        if was_scrolled != is_scrolled {
            SCROLLED_ELEMENTS.with(|count| {
                count.set(if is_scrolled {
                    count.get().saturating_add(1)
                } else {
                    count.get().saturating_sub(1)
                });
            });
        }
    }

    /// Sets the data for a text or comment node. No-op for other node kinds.
    pub fn set_data(&self, data: &str) {
        match &mut self.0.borrow_mut().data {
            NodeData::Text(text) => text.data = data.to_string(),
            NodeData::Comment(comment) => comment.data = data.to_string(),
            NodeData::ProcessingInstruction(pi) => pi.data = data.to_string(),
            _ => {}
        }
    }

    /// Returns the text/comment/doctype data for leaf nodes when applicable.
    pub fn data(&self) -> Option<String> {
        match &self.0.borrow().data {
            NodeData::Text(text) => Some(text.data().to_string()),
            NodeData::Comment(comment) => Some(comment.data().to_string()),
            NodeData::ProcessingInstruction(pi) => Some(pi.data.clone()),
            NodeData::DocumentType(doctype) => Some(doctype.name().to_string()),
            _ => None,
        }
    }

    /// Returns the public identifier for a document type node.
    pub fn public_id(&self) -> Option<String> {
        match &self.0.borrow().data {
            NodeData::DocumentType(doctype) => doctype.public_id().map(str::to_string),
            _ => None,
        }
    }

    /// Returns the system identifier for a document type node.
    pub fn system_id(&self) -> Option<String> {
        match &self.0.borrow().data {
            NodeData::DocumentType(doctype) => doctype.system_id().map(str::to_string),
            _ => None,
        }
    }

    /// Returns the first matching descendant element for a simple selector.
    ///
    /// Supported selectors are tag names, `#id`, `.class`, and simple
    /// attribute selectors such as `[name]`, `[name="value"]`, and
    /// `tag[name="value"]`.
    pub fn query_selector(&self, selector: &str) -> Option<NodeHandle> {
        if matches_selector(self, selector) {
            return Some(self.clone());
        }

        for child in self.child_nodes() {
            if let Some(found) = child.query_selector(selector) {
                return Some(found);
            }
        }

        None
    }
}

impl Node for NodeHandle {
    fn node_type(&self) -> NodeType {
        match &self.0.borrow().data {
            NodeData::Document(_) => NodeType::Document,
            NodeData::DocumentFragment | NodeData::ShadowRoot(_) => NodeType::DocumentFragment,
            NodeData::Element(_) => NodeType::Element,
            NodeData::Text(_) => NodeType::Text,
            NodeData::Comment(_) => NodeType::Comment,
            NodeData::ProcessingInstruction(_) => NodeType::ProcessingInstruction,
            NodeData::DocumentType(_) => NodeType::DocumentType,
        }
    }

    fn node_name(&self) -> String {
        match &self.0.borrow().data {
            NodeData::Document(_) => "#document".to_string(),
            NodeData::DocumentFragment | NodeData::ShadowRoot(_) => "#document-fragment".to_string(),
            NodeData::Element(element) if element.is_html() => element.tag_name.to_ascii_uppercase(),
            NodeData::Element(element) => element.tag_name.clone(),
            NodeData::Text(_) => "#text".to_string(),
            NodeData::Comment(_) => "#comment".to_string(),
            NodeData::ProcessingInstruction(pi) => pi.target.clone(),
            NodeData::DocumentType(doctype) => doctype.name.clone(),
        }
    }

    fn parent_node(&self) -> Option<NodeHandle> {
        self.0
            .borrow()
            .parent
            .as_ref()
            .and_then(Weak::upgrade)
            .map(NodeHandle)
    }

    fn child_nodes(&self) -> Vec<NodeHandle> {
        self.0.borrow().children.clone()
    }
}

/// Clears the stored scroll offset of `node` and every node beneath it,
/// including shadow trees and template contents.
fn clear_scroll_offsets(node: &NodeHandle) {
    if node.scroll_offset() != (0.0, 0.0) {
        node.set_scroll_offset(0.0, 0.0);
    }
    if let Some(content) = node.template_content() {
        clear_scroll_offsets(&content);
    }
    if let Some(root) = node.shadow_root() {
        clear_scroll_offsets(&root);
    }
    for child in node.child_nodes() {
        clear_scroll_offsets(&child);
    }
}

fn detach_from_parent(child: &NodeHandle) {
    let parent = child.parent_node();
    if let Some(parent) = parent {
        let _ = parent.remove_child(child);
    }
}

fn collect_slots(node: &NodeHandle, slots: &mut Vec<NodeHandle>) {
    for child in node.child_nodes() {
        if child.is_html_slot() {
            slots.push(child.clone());
        }
        collect_slots(&child, slots);
    }
}

/// Computes every slot assignment for one shadow host in a single pass over
/// the host's light children. This is the shared host-unit primitive used by
/// DOM accessors and flat-tree layout, avoiding a fresh shadow-tree walk for
/// each candidate slottable.
fn slot_assignments(root: &NodeHandle) -> Vec<(NodeHandle, Vec<NodeHandle>)> {
    let mut slots = Vec::new();
    collect_slots(root, &mut slots);
    let mut assignments: Vec<_> = slots
        .iter()
        .cloned()
        .map(|slot| (slot, Vec::new()))
        .collect();
    let Some(host) = root.shadow_host() else {
        return assignments;
    };
    for child in host.child_nodes() {
        if !child.is_slottable() {
            continue;
        }
        let requested_name = if child.node_type() == NodeType::Element {
            child.get_attribute("slot").unwrap_or_default()
        } else {
            String::new()
        };
        if let Some(index) = slots
            .iter()
            .position(|slot| slot.get_attribute("name").unwrap_or_default() == requested_name)
        {
            assignments[index].1.push(child);
        }
    }
    assignments
}

fn flatten_slotables(nodes: Vec<NodeHandle>, output: &mut Vec<NodeHandle>) {
    for node in nodes {
        if node.is_html_slot() {
            let assigned = node.assigned_nodes(false);
            let nested = if assigned.is_empty() {
                node.child_nodes()
            } else {
                assigned
            };
            flatten_slotables(nested, output);
        } else {
            output.push(node);
        }
    }
}

fn matches_selector(node: &NodeHandle, selector: &str) -> bool {
    let Some(attributes) = node.attributes() else {
        return false;
    };

    if let Some(parsed) = parse_attribute_selector(selector) {
        let tag_matches = parsed
            .tag_name
            .as_ref()
            .map(|tag_name| {
                node.tag_name()
                    .map(|actual| actual.eq_ignore_ascii_case(tag_name))
                    .unwrap_or(false)
            })
            .unwrap_or(true);
        if !tag_matches {
            return false;
        }

        return match attributes.get(parsed.attribute_name.as_str()) {
            Some(actual) => parsed
                .attribute_value
                .as_ref()
                .map(|expected| actual == expected)
                .unwrap_or(true),
            None => false,
        };
    }

    if let Some(id) = selector.strip_prefix('#') {
        return attributes
            .get("id")
            .map(|value| value == id)
            .unwrap_or(false);
    }

    if let Some(class_name) = selector.strip_prefix('.') {
        return attributes
            .get("class")
            .map(|value| {
                value
                    .split_ascii_whitespace()
                    .any(|class| class == class_name)
            })
            .unwrap_or(false);
    }

    node.tag_name()
        .map(|tag_name| tag_name.eq_ignore_ascii_case(selector))
        .unwrap_or(false)
}

struct AttributeSelector {
    tag_name: Option<String>,
    attribute_name: String,
    attribute_value: Option<String>,
}

fn parse_attribute_selector(selector: &str) -> Option<AttributeSelector> {
    let open = selector.find('[')?;
    let close = selector.rfind(']')?;
    if close <= open {
        return None;
    }

    let tag_name = selector[..open].trim();
    let body = selector[open + 1..close].trim();
    if body.is_empty() {
        return None;
    }

    let (attribute_name, attribute_value) = if let Some((name, value)) = body.split_once('=') {
        let trimmed_name = name.trim();
        if trimmed_name.is_empty() {
            return None;
        }
        let trimmed_value = value.trim().trim_matches('"').trim_matches('\'');
        (
            trimmed_name.to_ascii_lowercase(),
            Some(trimmed_value.to_string()),
        )
    } else {
        (body.to_ascii_lowercase(), None)
    };

    Some(AttributeSelector {
        tag_name: if tag_name.is_empty() {
            None
        } else {
            Some(tag_name.to_ascii_lowercase())
        },
        attribute_name,
        attribute_value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_basic_node_metadata() {
        let document = NodeHandle::document();
        let element = NodeHandle::element("div");
        let text = NodeHandle::text("hello");
        let comment = NodeHandle::comment("note");
        let doctype = NodeHandle::document_type("html", "", "");

        assert_eq!(document.node_type(), NodeType::Document);
        assert_eq!(document.node_name(), "#document");
        assert_eq!(element.node_type(), NodeType::Element);
        assert_eq!(element.node_name(), "DIV");
        assert_eq!(text.node_name(), "#text");
        assert_eq!(comment.node_name(), "#comment");
        assert_eq!(doctype.node_name(), "html");
    }

    #[test]
    fn append_child_sets_parent_and_child_order() {
        let document = NodeHandle::document();
        let html = NodeHandle::element("html");

        document.append_child(html.clone());

        assert_eq!(document.child_nodes(), vec![html.clone()]);
        assert_eq!(html.parent_node(), Some(document));
    }

    #[test]
    fn insert_before_places_node_before_reference() {
        let parent = NodeHandle::element("div");
        let first = NodeHandle::element("p");
        let second = NodeHandle::element("span");

        parent.append_child(second.clone());
        parent.insert_before(first.clone(), &second).unwrap();

        assert_eq!(parent.child_nodes(), vec![first, second]);
    }

    #[test]
    fn insert_before_reorders_an_earlier_sibling_before_the_reference() {
        let parent = NodeHandle::element("div");
        let moved = NodeHandle::element("a");
        let middle = NodeHandle::element("b");
        let reference = NodeHandle::element("c");
        let tail = NodeHandle::element("d");
        for child in [&moved, &middle, &reference, &tail] {
            parent.append_child(child.clone());
        }

        parent.insert_before(moved.clone(), &reference).unwrap();

        assert_eq!(
            parent.child_nodes(),
            vec![
                middle.clone(),
                moved.clone(),
                reference.clone(),
                tail.clone(),
            ]
        );
        parent
            .insert_before(reference.clone(), &reference)
            .unwrap();
        assert_eq!(parent.child_nodes(), vec![middle, moved, reference, tail]);
    }

    #[test]
    fn remove_child_detaches_node() {
        let parent = NodeHandle::element("div");
        let child = NodeHandle::text("hello");
        parent.append_child(child.clone());

        let removed = parent.remove_child(&child).unwrap();

        assert_eq!(removed, child);
        assert!(parent.child_nodes().is_empty());
        assert_eq!(child.parent_node(), None);
    }

    #[test]
    fn append_child_rejects_ancestor_cycles() {
        let grandparent = NodeHandle::element("section");
        let parent = NodeHandle::element("div");
        let child = NodeHandle::element("span");
        grandparent.append_child(parent.clone());
        parent.append_child(child.clone());

        // Appending an ancestor (grandparent) into a descendant (child) must be
        // refused so the tree stays acyclic; otherwise traversal would hang.
        child.append_child(grandparent.clone());

        assert!(child.child_nodes().is_empty());
        assert_eq!(grandparent.parent_node(), None);
        assert_eq!(grandparent.child_nodes(), vec![parent]);

        // Appending a node to itself is likewise refused.
        let solo = NodeHandle::element("p");
        solo.append_child(solo.clone());
        assert!(solo.child_nodes().is_empty());
    }

    #[test]
    fn insert_before_rejects_ancestor_cycles() {
        let parent = NodeHandle::element("div");
        let middle = NodeHandle::element("section");
        let leaf = NodeHandle::element("p");
        parent.append_child(middle.clone());
        middle.append_child(leaf.clone());

        // Inserting `parent` (an ancestor of `middle`) into `middle` would form
        // a cycle and must raise a hierarchy-request error.
        let result = middle.insert_before(parent.clone(), &leaf);
        assert_eq!(result, Err(DomError::HierarchyRequest));

        // Tree is unchanged.
        assert_eq!(middle.child_nodes(), vec![leaf]);
        assert_eq!(parent.parent_node(), None);
    }

    #[test]
    fn append_child_reparents_existing_nodes() {
        let first_parent = NodeHandle::element("section");
        let second_parent = NodeHandle::element("article");
        let child = NodeHandle::element("p");

        first_parent.append_child(child.clone());
        second_parent.append_child(child.clone());

        assert!(first_parent.child_nodes().is_empty());
        assert_eq!(second_parent.child_nodes(), vec![child.clone()]);
        assert_eq!(child.parent_node(), Some(second_parent));
    }

    #[test]
    fn shadow_slots_assign_named_default_and_first_matching_slot() {
        let host = NodeHandle::element("div");
        let named = NodeHandle::element("span");
        named.set_attribute("slot", "title");
        let text = NodeHandle::text("default");
        let unmatched = NodeHandle::element("p");
        unmatched.set_attribute("slot", "missing");
        host.append_child(named.clone());
        host.append_child(text.clone());
        host.append_child(unmatched.clone());

        let root = host.attach_shadow(ShadowRootMode::Open).unwrap();
        let first_named_slot = NodeHandle::element("slot");
        first_named_slot.set_attribute("name", "title");
        let duplicate_named_slot = NodeHandle::element("slot");
        duplicate_named_slot.set_attribute("name", "title");
        let default_slot = NodeHandle::element("slot");
        root.append_child(first_named_slot.clone());
        root.append_child(duplicate_named_slot.clone());
        root.append_child(default_slot.clone());

        assert_eq!(named.assigned_slot(), Some(first_named_slot.clone()));
        assert_eq!(text.assigned_slot(), Some(default_slot.clone()));
        assert_eq!(unmatched.assigned_slot(), None);
        assert_eq!(first_named_slot.assigned_nodes(false), vec![named]);
        assert!(duplicate_named_slot.assigned_nodes(false).is_empty());
        assert_eq!(default_slot.assigned_nodes(false), vec![text]);
    }

    #[test]
    fn slot_assignment_reacts_to_tree_and_attribute_changes() {
        let host = NodeHandle::element("div");
        let child = NodeHandle::element("span");
        host.append_child(child.clone());
        let root = host.attach_shadow(ShadowRootMode::Open).unwrap();
        let default_slot = NodeHandle::element("slot");
        let named_slot = NodeHandle::element("slot");
        named_slot.set_attribute("name", "named");
        root.append_child(default_slot.clone());
        root.append_child(named_slot.clone());

        assert_eq!(child.assigned_slot(), Some(default_slot.clone()));
        child.set_attribute("slot", "named");
        assert_eq!(child.assigned_slot(), Some(named_slot.clone()));
        named_slot.set_attribute("name", "other");
        assert_eq!(child.assigned_slot(), None);
        named_slot.set_attribute("name", "named");
        root.remove_child(&named_slot).unwrap();
        assert_eq!(child.assigned_slot(), None);
    }

    #[test]
    fn flat_tree_uses_assigned_nodes_and_fallback_content() {
        let host = NodeHandle::element("div");
        let assigned = NodeHandle::element("strong");
        assigned.set_attribute("slot", "content");
        host.append_child(assigned.clone());
        let root = host.attach_shadow(ShadowRootMode::Open).unwrap();
        let wrapper = NodeHandle::element("section");
        let slot = NodeHandle::element("slot");
        slot.set_attribute("name", "content");
        let fallback = NodeHandle::element("em");
        slot.append_child(fallback.clone());
        wrapper.append_child(slot.clone());
        root.append_child(wrapper.clone());

        assert_eq!(host.layout_child_nodes(), vec![wrapper.clone()]);
        assert_eq!(wrapper.layout_child_nodes(), vec![assigned.clone()]);
        assert_eq!(slot.assigned_nodes(true), vec![assigned]);

        host.remove_child(&host.child_nodes()[0]).unwrap();
        assert_eq!(slot.assigned_nodes(false), Vec::<NodeHandle>::new());
        assert_eq!(slot.assigned_nodes(true), vec![fallback.clone()]);
        assert_eq!(wrapper.layout_child_nodes(), vec![fallback]);
    }

    #[test]
    fn query_selector_matches_tag_id_and_class() {
        let document = NodeHandle::document();
        let html = NodeHandle::element("html");
        let body = NodeHandle::element("body");
        let main = NodeHandle::element("main");
        let title = NodeHandle::element("h1");

        main.set_attribute("id", "app");
        main.set_attribute("class", "hero primary");

        document.append_child(html.clone());
        html.append_child(body.clone());
        body.append_child(main.clone());
        main.append_child(title.clone());

        assert_eq!(document.query_selector("main"), Some(main.clone()));
        assert_eq!(document.query_selector("#app"), Some(main.clone()));
        assert_eq!(document.query_selector(".primary"), Some(main));
        assert_eq!(document.query_selector(".missing"), None);
    }

    #[test]
    fn query_selector_matches_simple_attribute_selectors() {
        let document = NodeHandle::document();
        let html = NodeHandle::element("html");
        let head = NodeHandle::element("head");
        let meta = NodeHandle::element("meta");

        meta.set_attribute("property", "og:image");
        meta.set_attribute("content", "https://example.com/image.jpg");

        document.append_child(html.clone());
        html.append_child(head.clone());
        head.append_child(meta.clone());

        assert_eq!(document.query_selector("[property]"), Some(meta.clone()));
        assert_eq!(
            document.query_selector(r#"meta[property="og:image"]"#),
            Some(meta.clone())
        );
        assert_eq!(
            document.query_selector(r#"[content="https://example.com/image.jpg"]"#),
            Some(meta)
        );
    }

    #[test]
    fn element_attributes_are_normalized_to_lowercase() {
        let element = NodeHandle::element("div");
        element.set_attribute("DATA-ID", "42");

        let attributes = element.attributes().unwrap();
        assert_eq!(attributes.get("data-id"), Some(&"42".to_string()));
    }

    #[test]
    fn gets_single_element_attribute_without_losing_empty_values() {
        let element = NodeHandle::element("div");
        element.set_attribute("id", "example");
        element.set_attribute("style", "");

        assert_eq!(element.get_attribute("id"), Some("example".to_string()));
        assert_eq!(element.get_attribute("missing"), None);
        assert_eq!(element.get_attribute("style"), Some(String::new()));
    }

    #[test]
    fn gets_attributes_by_exact_name_then_ascii_lowercase_fallback() {
        let element = NodeHandle::element("svg");
        element.set_xml_attribute("viewBox", "0 0 10 10");
        element.set_attribute("id", "example");
        let xml = NodeHandle::xml_element("root", None);
        xml.set_attribute("lowercase", "value");

        assert_eq!(
            element.get_attribute("viewBox"),
            Some("0 0 10 10".to_string())
        );
        assert_eq!(element.get_attribute("ID"), Some("example".to_string()));
        assert_eq!(xml.get_attribute("LOWERCASE"), None);
    }

    #[test]
    fn preserves_element_and_attribute_namespace_metadata() {
        let html = NodeHandle::html_element_ns("P:DIV", "http://www.w3.org/1999/xhtml");
        assert_eq!(html.tag_name().as_deref(), Some("P:DIV"));
        assert_eq!(html.prefix().as_deref(), Some("P"));
        assert_eq!(html.local_name().as_deref(), Some("DIV"));

        let xml = NodeHandle::xml_element("p:Root", Some("urn:root".to_string()));
        xml.set_attribute("MixedCase", "plain");
        xml.set_xml_attribute_ns("a:item", Some("urn:attribute".to_string()), "item", "value");
        assert_eq!(xml.get_attribute("mixedcase"), None);
        assert_eq!(xml.attribute_records().unwrap(), vec![
            ("MixedCase".into(), None, "MixedCase".into(), "plain".into()),
            ("a:item".into(), Some("urn:attribute".into()), "item".into(), "value".into()),
        ]);
        xml.set_attribute("a:item", "updated");
        assert!(xml.attribute_records().unwrap().contains(&(
            "a:item".into(),
            Some("urn:attribute".into()),
            "item".into(),
            "updated".into(),
        )));
        xml.remove_attribute("mixedcase");
        assert!(xml.get_attribute("MixedCase").is_some());
        xml.remove_attribute("MixedCase");
        xml.remove_xml_attribute("a:item");
        assert!(xml.attribute_records().unwrap().is_empty());
    }

    #[test]
    fn scroll_offset_defaults_to_zero_and_round_trips_on_elements() {
        let element = NodeHandle::element("div");
        let text = NodeHandle::text("hello");

        assert_eq!(element.scroll_offset(), (0.0, 0.0));
        assert_eq!(text.scroll_offset(), (0.0, 0.0));

        element.set_scroll_offset(12.5, 30.0);
        assert_eq!(element.scroll_offset(), (12.5, 30.0));

        // Non-elements have no scrolling box, so the setter is a no-op.
        text.set_scroll_offset(4.0, 5.0);
        assert_eq!(text.scroll_offset(), (0.0, 0.0));
    }

    #[test]
    fn scroll_offset_resets_when_a_node_is_detached() {
        let parent = NodeHandle::element("div");
        let child = NodeHandle::element("div");
        let grandchild = NodeHandle::element("div");
        parent.append_child(child.clone());
        child.append_child(grandchild.clone());
        child.set_scroll_offset(10.0, 20.0);
        grandchild.set_scroll_offset(3.0, 4.0);

        parent.remove_child(&child).unwrap();

        // Removing a subtree destroys its boxes, so every offset inside it is
        // gone; re-inserting starts from the top again.
        assert_eq!(child.scroll_offset(), (0.0, 0.0));
        assert_eq!(grandchild.scroll_offset(), (0.0, 0.0));
        parent.append_child(child.clone());
        assert_eq!(child.scroll_offset(), (0.0, 0.0));
    }

    #[test]
    fn scroll_offset_resets_when_a_node_moves_or_is_reordered() {
        let first_parent = NodeHandle::element("div");
        let second_parent = NodeHandle::element("div");
        let moved = NodeHandle::element("div");
        let reordered = NodeHandle::element("div");
        let sibling = NodeHandle::element("div");
        first_parent.append_child(moved.clone());
        first_parent.append_child(reordered.clone());
        first_parent.append_child(sibling.clone());
        moved.set_scroll_offset(1.0, 2.0);
        reordered.set_scroll_offset(3.0, 4.0);

        // Both re-parenting and reordering within one parent detach the node
        // first, which drops the scroll offset.
        second_parent.append_child(moved.clone());
        first_parent.insert_before(reordered.clone(), &sibling).unwrap();

        assert_eq!(moved.scroll_offset(), (0.0, 0.0));
        assert_eq!(reordered.scroll_offset(), (0.0, 0.0));
    }

    #[test]
    fn scrolled_element_tracking_reports_whether_any_offset_is_set() {
        let element = NodeHandle::element("div");
        let scrolled_before = any_element_scrolled();

        element.set_scroll_offset(0.0, 5.0);
        assert!(
            any_element_scrolled(),
            "a non-zero offset must be observable to the paint fast path"
        );

        element.set_scroll_offset(0.0, 0.0);
        assert_eq!(
            any_element_scrolled(),
            scrolled_before,
            "clearing the offset must undo the tracking increment"
        );
    }
}
