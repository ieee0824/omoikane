//! Allocation identity for collector-owned ephemeron sets.
//!
//! A trait-object pointer may carry different vtables at different coercion
//! sites. Equality and hashing use only its allocation address. The original
//! wide pointer is retained unchanged for dispatch, including its provenance;
//! no pointer is reconstructed from an integer address. Sweep must remove an
//! entry before freeing its allocation, as with the collector's other sets.

use crate::{EphemeronPointer, PointerSet};
use std::hash::{Hash, Hasher};

#[derive(Clone, Copy)]
struct Identity(EphemeronPointer);

impl PartialEq for Identity {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::addr_eq(self.0.as_ptr(), other.0.as_ptr())
    }
}
impl Eq for Identity {}
impl Hash for Identity {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.cast::<()>().hash(state);
    }
}

/// An internal allocation-address set with the original dispatch pointers.
#[derive(Default)]
pub(crate) struct EphemeronSet(PointerSet<Identity>);

impl EphemeronSet {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn insert(&mut self, pointer: EphemeronPointer) -> bool {
        self.0.insert(Identity(pointer))
    }

    pub(crate) fn remove(&mut self, pointer: &EphemeronPointer) -> bool {
        self.0.remove(&Identity(*pointer))
    }

    #[cfg(test)]
    pub(crate) fn contains(&self, pointer: &EphemeronPointer) -> bool {
        self.0.contains(&Identity(*pointer))
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &EphemeronPointer> {
        self.0.iter().map(|identity| &identity.0)
    }

    pub(crate) fn retain(&mut self, mut live: impl FnMut(&EphemeronPointer) -> bool) {
        self.0.retain(|identity| live(&identity.0));
    }
}

impl FromIterator<EphemeronPointer> for EphemeronSet {
    fn from_iter<T: IntoIterator<Item = EphemeronPointer>>(iter: T) -> Self {
        Self(iter.into_iter().map(Identity).collect())
    }
}

impl Extend<EphemeronPointer> for EphemeronSet {
    fn extend<T: IntoIterator<Item = EphemeronPointer>>(&mut self, iter: T) {
        self.0.extend(iter.into_iter().map(Identity));
    }
}
