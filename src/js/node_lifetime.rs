//! GC ownership of native DOM trees and their canonical JavaScript wrappers.
//!
//! A wrapper privately retains a JS group for its (Document, Realm). Active
//! document/Realm pairs are host roots. Retired and inert groups survive only
//! through real JS references, so their wrapper cycles are garbage collectible.
//! Native records use Rc/Weak; no native weak-GC allocation owns their lifetime.

use super::*;
use boa_engine::JsData;
use boa_gc::{GcRefCell, Rooted};
use crate::dom::WeakNodeHandle;

type DocumentNodes = Rc<RefCell<HashMap<usize, NodeHandle>>>;
type GroupKey = (usize, usize);
type Group = Rc<GcRefCell<WrapperGroup>>;
type Lease = Rc<GcRefCell<JsObject>>;

struct WrapperGroup {
    _document: DocumentNodes,
    object: Option<JsObject>,
    wrappers: HashMap<usize, WrapperEntry>,
}

struct WrapperEntry {
    wrapper: JsObject,
    lease: Lease,
}

#[derive(Finalize, JsData)]
struct GroupData(Group);

impl Finalize for WrapperGroup {}

unsafe impl Trace for WrapperGroup {
    unsafe fn trace(&self, tracer: &mut Tracer) {
        if let Some(object) = &self.object {
            unsafe { object.trace(tracer) };
        }
        for entry in self.wrappers.values() {
            // The wrapper's private WeakMap value traces LeaseData. Only that
            // JS object may install the lease cell's write-barrier owner;
            // tracing the same cell here would overwrite it during adoption.
            unsafe { entry.wrapper.trace(tracer) };
        }
    }
    fn run_finalizer(&self) {}
}

// Tracing the GcRefCell installs the owning JS object's generational write
// barrier. Native mutations must use this cell, even though Rc owns its storage.
unsafe impl Trace for GroupData {
    unsafe fn trace(&self, tracer: &mut Tracer) {
        unsafe { self.0.as_ref().trace(tracer) };
    }
    fn run_finalizer(&self) {}
}

#[derive(Finalize, JsData)]
struct LeaseData(Lease);

unsafe impl Trace for LeaseData {
    unsafe fn trace(&self, tracer: &mut Tracer) {
        unsafe { self.0.as_ref().trace(tracer) };
    }
    fn run_finalizer(&self) {}
}

/// A temporary root protects a group while a native binding allocates objects.
struct GroupRoot {
    group: Group,
    object: Rooted<JsObject>,
}

#[derive(Default)]
pub(super) struct NodeLifetimes {
    documents: HashMap<usize, Weak<RefCell<HashMap<usize, NodeHandle>>>>,
    groups: HashMap<GroupKey, Weak<GcRefCell<WrapperGroup>>>,
    active: HashMap<GroupKey, Group>,
    nodes: HashMap<usize, WeakNodeHandle>,
    owners: HashMap<usize, usize>,
}

impl Finalize for NodeLifetimes {}

unsafe impl Trace for NodeLifetimes {
    unsafe fn trace(&self, tracer: &mut Tracer) {
        for group in self.active.values() {
            if let Some(object) = &group.borrow().object {
                unsafe { object.trace(tracer) };
            }
        }
    }
    fn run_finalizer(&self) {}
}

impl HostState {
    pub(super) fn document_is_active(&self, id: usize) -> bool {
        id == self.document.identity()
            || self
                .iframe_documents
                .values()
                .any(|entry| entry.document.identity() == id)
    }

    pub(super) fn node_is_in_active_document(&self, node: &NodeHandle) -> bool {
        document_root_for_node(node)
            .is_some_and(|document| self.document_is_active(document.identity()))
    }

    fn document_nodes(&mut self, document: &NodeHandle) -> DocumentNodes {
        let id = document.identity();
        if let Some(nodes) = self
            .node_lifetimes
            .documents
            .get(&id)
            .and_then(Weak::upgrade)
        {
            return nodes;
        }
        let nodes = Rc::new(RefCell::new(HashMap::new()));
        self.node_lifetimes
            .documents
            .insert(id, Rc::downgrade(&nodes));
        let mut ids = HashSet::new();
        Self::collect_tree_ids(document, &mut ids);
        for node_id in ids {
            if let Some(node) = self.get_node(node_id) {
                self.enroll_node(&node, id, &nodes);
            }
        }
        nodes
    }

    fn wrapper_group(
        &mut self,
        document_id: usize,
        realm_id: usize,
        document: &DocumentNodes,
    ) -> GroupRoot {
        let key = (document_id, realm_id);
        if let Some(group) = self.node_lifetimes.groups.get(&key).and_then(Weak::upgrade) {
            let object = group.borrow().object.as_ref().unwrap().clone();
            return GroupRoot {
                group,
                object: Rooted::new(object),
            };
        }
        let group = Rc::new(GcRefCell::new(WrapperGroup {
            _document: document.clone(),
            object: None,
            wrappers: HashMap::new(),
        }));
        let object = JsObject::from_proto_and_data(None, GroupData(group.clone()));
        group.borrow_mut().object = Some(object.clone());
        let object = Rooted::new(object);
        self.node_lifetimes
            .groups
            .insert(key, Rc::downgrade(&group));
        if self.document_is_active(document_id) && self.document_is_active(realm_id) {
            self.node_lifetimes.active.insert(key, group.clone());
        }
        GroupRoot { group, object }
    }

    fn enroll_node(&mut self, node: &NodeHandle, document_id: usize, document: &DocumentNodes) {
        let id = node.identity();
        if let Some(previous_id) = self.node_lifetimes.owners.insert(id, document_id)
            && previous_id != document_id
        {
            if let Some(previous) = self
                .node_lifetimes
                .documents
                .get(&previous_id)
                .and_then(Weak::upgrade)
            {
                previous.borrow_mut().remove(&id);
            }
            // Adoption updates every Realm's alias, including wrappers not
            // present in the calling Realm's private JS cache.
            let groups: Vec<_> = self
                .node_lifetimes
                .groups
                .iter()
                .filter_map(|((owner, realm), group)| {
                    if *owner != previous_id {
                        return None;
                    }
                    let group = group.upgrade()?;
                    let has_wrapper = group.borrow().wrappers.contains_key(&id);
                    has_wrapper.then_some((*realm, group))
                })
                .collect();
            if !groups.is_empty() {
                // Rc upgrades protect native records, not their JS edges. Root
                // all source groups before allocating any destination group.
                let _roots = Rooted::new(
                    groups
                        .iter()
                        .map(|(_, group)| group.borrow().object.as_ref().unwrap().clone())
                        .collect::<Vec<_>>(),
                );
                for (realm, previous) in groups {
                    let next = self.wrapper_group(document_id, realm, document);
                    let entry = previous.borrow_mut().wrappers.remove(&id).unwrap();
                    *entry.lease.borrow_mut() = (*next.object).clone();
                    next.group.borrow_mut().wrappers.insert(id, entry);
                }
            }
        }
        self.node_lifetimes.nodes.insert(id, node.downgrade());
        document.borrow_mut().insert(id, node.clone());
        if self.document_is_active(document_id) {
            self.nodes.insert(id, node.clone());
        } else {
            self.nodes.remove(&id);
        }
    }

    /// Enroll parser-created nodes without creating JavaScript wrappers.
    pub(super) fn enroll_registered_tree(&mut self, node: &NodeHandle, owner: Option<usize>) {
        let owner = document_root_for_node(node)
            .map(|document| document.identity())
            .or_else(|| self.node_lifetimes.owners.get(&node.identity()).copied())
            .or(owner);
        if let Some(owner) = owner
            && let Some(document) = self
                .node_lifetimes
                .documents
                .get(&owner)
                .and_then(Weak::upgrade)
        {
            self.enroll_node(node, owner, &document);
        }
        if let Some(content) = node.template_content() {
            self.enroll_registered_tree(&content, owner);
        }
        if let Some(root) = node.shadow_root() {
            self.enroll_registered_tree(&root, owner);
        }
        for child in node.child_nodes() {
            self.enroll_registered_tree(&child, owner);
        }
    }

    pub(super) fn retain_node_wrapper(
        &mut self,
        node: NodeHandle,
        wrapper: JsObject,
        realm_id: usize,
    ) -> JsObject {
        let owner = document_root_for_node(&node)
            .or_else(|| {
                self.node_lifetimes
                    .owners
                    .get(&node.identity())
                    .and_then(|id| self.get_node(*id))
            })
            .unwrap_or_else(|| self.document.clone());
        let document = self.document_nodes(&owner);
        let enrolled = self.node_lifetimes.owners.contains_key(&node.identity());
        self.enroll_node(&node, owner.identity(), &document);
        if !enrolled {
            self.enroll_registered_tree(&node, Some(owner.identity()));
        }
        let group = self.wrapper_group(owner.identity(), realm_id, &document);
        let lease = Rc::new(GcRefCell::new((*group.object).clone()));
        group.group.borrow_mut().wrappers.insert(
            node.identity(),
            WrapperEntry {
                wrapper,
                lease: lease.clone(),
            },
        );
        JsObject::from_proto_and_data(None, LeaseData(lease))
    }

    pub(super) fn set_node_lifetime_owner(&mut self, node: &NodeHandle, document: &NodeHandle) {
        if self.node_lifetimes.owners.get(&node.identity()) == Some(&document.identity()) {
            return;
        }
        let nodes = self.document_nodes(document);
        self.enroll_node(node, document.identity(), &nodes);
    }

    pub(super) fn retired_document_node_ids(&mut self, document_id: usize) -> Vec<usize> {
        self.node_lifetimes
            .active
            .retain(|(owner, realm), _| *owner != document_id && *realm != document_id);
        self.node_lifetimes
            .owners
            .iter()
            .filter_map(|(node_id, owner)| (*owner == document_id).then_some(*node_id))
            .collect()
    }

    pub(super) fn retained_node(&self, id: usize) -> Option<NodeHandle> {
        let owner = self.node_lifetimes.owners.get(&id)?;
        if self.node_lifetimes.documents.get(owner)?.strong_count() == 0 {
            return None;
        }
        self.node_lifetimes
            .nodes
            .get(&id)
            .and_then(WeakNodeHandle::upgrade)
    }

    /// Style resolvers can own native DOM handles. A document record's death,
    /// not the DOM Rc count, determines cache reclamation after JS collection.
    pub(super) fn sweep_node_lifetimes(&mut self) {
        self.node_lifetimes
            .groups
            .retain(|_, group| group.strong_count() != 0);
        let dead: HashSet<_> = self
            .node_lifetimes
            .documents
            .iter()
            .filter_map(|(id, document)| (document.strong_count() == 0).then_some(*id))
            .collect();
        if dead.is_empty() {
            return;
        }
        self.node_lifetimes
            .documents
            .retain(|id, _| !dead.contains(id));
        self.document_styles.retain(|id, _| !dead.contains(id));
        self.write_parsers.retain(|id, _| !dead.contains(id));
        let ids: Vec<_> = self
            .node_lifetimes
            .owners
            .iter()
            .filter_map(|(id, owner)| dead.contains(owner).then_some(*id))
            .collect();
        for id in ids {
            self.node_lifetimes.nodes.remove(&id);
            self.node_lifetimes.owners.remove(&id);
            self.nodes.remove(&id);
            self.adopted_stylesheets.remove(&id);
        }
    }
}

pub(super) fn retain_node_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    let wrapper = args
        .get(1)
        .and_then(JsValue::as_object)
        .ok_or_else(|| JsNativeError::typ().with_message("node wrapper required"))?;
    let realm_id = context
        .realm()
        .host_defined()
        .get::<ModuleDocumentId>()
        .map(|owner| owner.0);
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        let node = state
            .get_node(id)
            .ok_or_else(|| JsNativeError::reference().with_message("node not found"))?;
        let realm_id = realm_id.unwrap_or_else(|| state.document.identity());
        Ok(state.retain_node_wrapper(node, wrapper, realm_id).into())
    })
}

pub(super) fn set_owner_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    let owner_id = parse_node_id(args.get(1), context)?;
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        let node = state
            .get_node(id)
            .ok_or_else(|| JsNativeError::reference().with_message("node not found"))?;
        let document = state
            .get_node(owner_id)
            .filter(|node| node.node_type() == NodeType::Document)
            .ok_or_else(|| JsNativeError::typ().with_message("owner must be a document"))?;
        state.set_node_lifetime_owner(&node, &document);
        Ok(JsValue::undefined())
    })
}

pub(super) fn collected_nodes_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let candidates = args
        .first()
        .and_then(JsValue::as_object)
        .ok_or_else(|| JsNativeError::typ().with_message("node ids required"))?;
    let length = candidates
        .get(js_string!("length"), context)?
        .to_length(context)?;
    let mut ids = Vec::new();
    for index in 0..length {
        ids.push(parse_node_id(
            Some(&candidates.get(index, context)?),
            context,
        )?);
    }
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        state.sweep_node_lifetimes();
        let ids = ids
            .into_iter()
            .filter(|id| state.get_node(*id).is_none())
            .map(|id| JsValue::from(id as f64));
        Ok(boa_engine::object::builtins::JsArray::from_iter(ids, context).into())
    })
}

#[cfg(test)]
impl NodeLifetimes {
    pub(super) fn document_count(&self) -> usize {
        self.documents.len()
    }
    pub(super) fn node_count(&self) -> usize {
        self.nodes.len()
    }
}
