//! DOM primitives.
//!
//! The DOM layer uses reference-counted node handles so later parser and style
//! phases can share and mutate the same tree.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::rc::{Rc, Weak};

/// A handle to a DOM node.
#[derive(Clone, Debug)]
pub struct NodeHandle(Rc<RefCell<NodeInner>>);

impl PartialEq for NodeHandle {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for NodeHandle {}

#[derive(Debug)]
struct NodeInner {
    parent: Option<Weak<RefCell<NodeInner>>>,
    children: Vec<NodeHandle>,
    data: NodeData,
}

#[derive(Debug, Clone)]
enum NodeData {
    Document(Document),
    DocumentFragment,
    Element(Element),
    Text(Text),
    Comment(Comment),
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
    DocumentType,
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

/// A DOM element node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    tag_name: String,
    namespace_uri: Option<String>,
    prefix: Option<String>,
    local_name: String,
    html: bool,
    attributes: BTreeMap<String, String>,
    checked: bool,
    dirty_checkedness: bool,
}

impl Element {
    /// Creates a new element payload.
    pub fn new(tag_name: impl Into<String>) -> Self {
        Self {
            tag_name: tag_name.into().to_ascii_lowercase(),
            namespace_uri: None,
            prefix: None,
            local_name: String::new(),
            html: true,
            attributes: BTreeMap::new(),
            checked: false,
            dirty_checkedness: false,
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
            checked: false,
            dirty_checkedness: false,
        }
    }

    /// Returns the normalized tag name.
    pub fn tag_name(&self) -> &str {
        &self.tag_name
    }

    pub fn namespace_uri(&self) -> Option<&str> { self.namespace_uri.as_deref() }
    pub fn prefix(&self) -> Option<&str> { self.prefix.as_deref() }
    pub fn local_name(&self) -> &str {
        if self.html { &self.tag_name } else { &self.local_name }
    }
    pub fn is_html(&self) -> bool { self.html }

    /// Returns the element attributes.
    pub fn attributes(&self) -> &BTreeMap<String, String> {
        &self.attributes
    }
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
            parent: None,
            children: Vec::new(),
            data,
        })))
    }

    /// Returns a stable identity for this node handle.
    pub(crate) fn identity(&self) -> usize {
        Rc::as_ptr(&self.0) as usize
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

        let index = self
            .0
            .borrow()
            .children
            .iter()
            .position(|child| child == reference_child)
            .ok_or(DomError::ReferenceChildNotFound)?;

        detach_from_parent(&new_child);
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

    /// Sets an attribute on an element node. No-op for other node kinds.
    pub fn set_attribute(&self, name: impl Into<String>, value: impl Into<String>) {
        if let NodeData::Element(element) = &mut self.0.borrow_mut().data {
            let name = name.into().to_ascii_lowercase();
            if name == "checked" && !element.dirty_checkedness {
                element.checked = true;
            }
            element
                .attributes
                .insert(name, value.into());
        }
    }


    /// Sets an XML attribute without HTML ASCII case folding.
    pub fn set_xml_attribute(&self, name: impl Into<String>, value: impl Into<String>) {
        if let NodeData::Element(element) = &mut self.0.borrow_mut().data {
            element.attributes.insert(name.into(), value.into());
        }
    }

    pub fn namespace_uri(&self) -> Option<String> {
        match &self.0.borrow().data {
            NodeData::Element(element) => element.namespace_uri().map(str::to_string),
            _ => None,
        }
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
            let name = name.to_ascii_lowercase();
            element.attributes.remove(&name);
            if name == "checked" && !element.dirty_checkedness {
                element.checked = false;
            }
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

    /// Sets the data for a text or comment node. No-op for other node kinds.
    pub fn set_data(&self, data: &str) {
        match &mut self.0.borrow_mut().data {
            NodeData::Text(text) => text.data = data.to_string(),
            NodeData::Comment(comment) => comment.data = data.to_string(),
            _ => {}
        }
    }

    /// Returns the text/comment/doctype data for leaf nodes when applicable.
    pub fn data(&self) -> Option<String> {
        match &self.0.borrow().data {
            NodeData::Text(text) => Some(text.data().to_string()),
            NodeData::Comment(comment) => Some(comment.data().to_string()),
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
            NodeData::DocumentFragment => NodeType::DocumentFragment,
            NodeData::Element(_) => NodeType::Element,
            NodeData::Text(_) => NodeType::Text,
            NodeData::Comment(_) => NodeType::Comment,
            NodeData::DocumentType(_) => NodeType::DocumentType,
        }
    }

    fn node_name(&self) -> String {
        match &self.0.borrow().data {
            NodeData::Document(_) => "#document".to_string(),
            NodeData::DocumentFragment => "#document-fragment".to_string(),
            NodeData::Element(element) if element.is_html() => element.tag_name.to_ascii_uppercase(),
            NodeData::Element(element) => element.tag_name.clone(),
            NodeData::Text(_) => "#text".to_string(),
            NodeData::Comment(_) => "#comment".to_string(),
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

fn detach_from_parent(child: &NodeHandle) {
    let parent = child.parent_node();
    if let Some(parent) = parent {
        let _ = parent.remove_child(child);
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
}
