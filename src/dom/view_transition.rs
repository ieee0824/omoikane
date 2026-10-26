//! Document-owned state shared by View Transitions and CSS selector matching.

use super::{NodeData, NodeHandle};

impl NodeHandle {
    /// Returns whether this document currently owns an active view transition.
    pub(crate) fn document_has_active_view_transition(&self) -> Option<bool> {
        match &self.0.borrow().data {
            NodeData::Document(_, metadata) => Some(metadata.active_view_transition),
            _ => None,
        }
    }

    /// Updates the document's active view-transition state.
    ///
    /// The JavaScript lifecycle owner is responsible for invalidating styles
    /// when this returns `true`.
    pub(crate) fn set_document_active_view_transition(&self, active: bool) -> bool {
        let mut inner = self.0.borrow_mut();
        let NodeData::Document(_, metadata) = &mut inner.data else {
            return false;
        };
        let changed = metadata.active_view_transition != active;
        metadata.active_view_transition = active;
        changed
    }
}
