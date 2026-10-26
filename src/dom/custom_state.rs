//! Owned custom state names shared by JavaScript mutations and CSS matching.
use super::{NodeData, NodeHandle};
use std::collections::HashSet;

/// Allocated only when an element first receives a custom state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct CustomStates(HashSet<Vec<u16>>);

impl NodeHandle {
    /// Matches an already-decoded CSS state name without running author code.
    pub(crate) fn has_custom_state(&self, name: &[u16]) -> bool {
        let node = self.0.borrow();
        let NodeData::Element(element) = &node.data else {
            return false;
        };
        element.custom_element_defined == Some(true)
            && element
                .custom_states
                .as_ref()
                .is_some_and(|states| states.0.contains(name))
    }

    /// Updates native membership, returning whether style matching changed.
    /// The caller owns document style invalidation after a true result.
    pub(crate) fn update_custom_state(&self, name: Vec<u16>, present: bool) -> bool {
        let mut node = self.0.borrow_mut();
        let NodeData::Element(element) = &mut node.data else {
            return false;
        };
        if present {
            element
                .custom_states
                .get_or_insert_with(Default::default)
                .0
                .insert(name)
        } else {
            element
                .custom_states
                .as_mut()
                .is_some_and(|states| states.0.remove(name.as_slice()))
        }
    }

    /// Clears native membership; JavaScript keeps its live ordered set object.
    pub(crate) fn clear_custom_states(&self) -> bool {
        let mut node = self.0.borrow_mut();
        let NodeData::Element(element) = &mut node.data else {
            return false;
        };
        element
            .custom_states
            .take()
            .is_some_and(|states| !states.0.is_empty())
    }
}
