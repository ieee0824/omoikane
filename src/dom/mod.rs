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
    Element(Element),
    Text(Text),
    Comment(Comment),
    DocumentType(DocumentType),
}

/// DOM node kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeType {
    Document,
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
    attributes: BTreeMap<String, String>,
}

impl Element {
    /// Creates a new element payload.
    pub fn new(tag_name: impl Into<String>) -> Self {
        Self {
            tag_name: tag_name.into().to_ascii_lowercase(),
            attributes: BTreeMap::new(),
        }
    }

    /// Returns the normalized tag name.
    pub fn tag_name(&self) -> &str {
        &self.tag_name
    }

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
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            public_id: None,
            system_id: None,
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
}

impl fmt::Display for DomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReferenceChildNotFound => write!(f, "reference child not found"),
            Self::ChildNotFound => write!(f, "child not found"),
        }
    }
}

impl std::error::Error for DomError {}

impl NodeHandle {
    /// Creates a document node.
    pub fn document() -> Self {
        Self::new(NodeData::Document(Document))
    }

    /// Creates an element node.
    pub fn element(tag_name: impl Into<String>) -> Self {
        Self::new(NodeData::Element(Element::new(tag_name)))
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
    pub fn document_type(name: impl Into<String>) -> Self {
        Self::new(NodeData::DocumentType(DocumentType::new(name)))
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
    pub fn append_child(&self, child: NodeHandle) {
        detach_from_parent(&child);
        child.0.borrow_mut().parent = Some(Rc::downgrade(&self.0));
        self.0.borrow_mut().children.push(child);
    }

    /// Inserts `new_child` before `reference_child`.
    pub fn insert_before(
        &self,
        new_child: NodeHandle,
        reference_child: &NodeHandle,
    ) -> Result<(), DomError> {
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
            element
                .attributes
                .insert(name.into().to_ascii_lowercase(), value.into());
        }
    }

    /// Returns the text/comment/doctype data for leaf nodes when applicable.
    pub fn data(&self) -> Option<String> {
        match &self.0.borrow().data {
            NodeData::Text(text) => Some(text.data.clone()),
            NodeData::Comment(comment) => Some(comment.data.clone()),
            NodeData::DocumentType(doctype) => Some(doctype.name.clone()),
            _ => None,
        }
    }

    /// Returns the first matching descendant element for a simple selector.
    ///
    /// Supported selectors are tag names, `#id`, and `.class`.
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
            NodeData::Element(_) => NodeType::Element,
            NodeData::Text(_) => NodeType::Text,
            NodeData::Comment(_) => NodeType::Comment,
            NodeData::DocumentType(_) => NodeType::DocumentType,
        }
    }

    fn node_name(&self) -> String {
        match &self.0.borrow().data {
            NodeData::Document(_) => "#document".to_string(),
            NodeData::Element(element) => element.tag_name.to_ascii_uppercase(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_basic_node_metadata() {
        let document = NodeHandle::document();
        let element = NodeHandle::element("div");
        let text = NodeHandle::text("hello");
        let comment = NodeHandle::comment("note");
        let doctype = NodeHandle::document_type("html");

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
    fn element_attributes_are_normalized_to_lowercase() {
        let element = NodeHandle::element("div");
        element.set_attribute("DATA-ID", "42");

        let attributes = element.attributes().unwrap();
        assert_eq!(attributes.get("data-id"), Some(&"42".to_string()));
    }
}
