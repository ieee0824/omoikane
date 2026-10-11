//! Owned child handles with stable slots and ordinary sibling order.
//!
//! Slots belong to one parent. Removing a child releases its handle and makes
//! its slot reusable; neighboring nodes never retain one another. Slot lookup
//! must also validate handle identity because IDs can be reused or reparented.

use super::NodeHandle;
#[cfg(test)]
use super::record_sibling_position_work;

const NONE: usize = usize::MAX;

#[derive(Debug, Default)]
pub(super) struct Children(Option<Box<Storage>>);

#[derive(Debug)]
struct Storage {
    slots: Vec<Slot>,
    first: usize,
    last: usize,
    free: usize,
    len: usize,
}

#[derive(Debug)]
struct Slot {
    node: Option<NodeHandle>,
    previous: usize,
    next: usize,
}

impl Default for Storage {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            first: NONE,
            last: NONE,
            free: NONE,
            len: 0,
        }
    }
}

impl Children {
    pub(super) fn len(&self) -> usize {
        self.0.as_ref().map_or(0, |storage| storage.len)
    }

    pub(super) fn get_slot(&self, slot: usize) -> Option<&NodeHandle> {
        self.0.as_ref()?.slots.get(slot)?.node.as_ref()
    }

    pub(super) fn first(&self) -> Option<&NodeHandle> {
        self.get_slot(self.0.as_ref()?.first)
    }

    pub(super) fn last(&self) -> Option<&NodeHandle> {
        self.get_slot(self.0.as_ref()?.last)
    }

    pub(super) fn adjacent(&self, slot: usize, previous: bool) -> Option<&NodeHandle> {
        let entry = self.0.as_ref()?.slots.get(slot)?;
        entry.node.as_ref()?;
        self.get_slot(if previous { entry.previous } else { entry.next })
    }

    pub(super) fn push(&mut self, node: NodeHandle) {
        let storage = self.0.get_or_insert_with(|| Box::new(Storage::default()));
        storage.insert_between(node, storage.last, NONE);
    }

    pub(super) fn insert_before(&mut self, node: NodeHandle, reference: usize) {
        let storage = self.0.as_mut().expect("reference belongs to parent");
        assert!(storage.slots[reference].node.is_some());
        let previous = storage.slots[reference].previous;
        storage.insert_between(node, previous, reference);
    }

    pub(super) fn remove(&mut self, slot: usize) -> NodeHandle {
        let storage = self.0.as_mut().expect("child belongs to parent");
        let entry = &mut storage.slots[slot];
        let node = entry.node.take().expect("live child slot");
        let (previous, next) = (entry.previous, entry.next);
        entry.next = storage.free;
        storage.free = slot;
        #[cfg(test)]
        record_sibling_position_work(1);
        if previous == NONE {
            storage.first = next;
        } else {
            storage.slots[previous].next = next;
            #[cfg(test)]
            record_sibling_position_work(1);
        }
        if next == NONE {
            storage.last = previous;
        } else {
            storage.slots[next].previous = previous;
            #[cfg(test)]
            record_sibling_position_work(1);
        }
        storage.len -= 1;
        node
    }

    /// This borrowed iterator is confined to the parent's immutable DOM view.
    /// It is never stored in DOM state or returned through a public Node API.
    pub(super) fn iter(&self) -> impl DoubleEndedIterator<Item = &NodeHandle> + ExactSizeIterator {
        let (slots, first, last, remaining) =
            self.0.as_ref().map_or((&[][..], NONE, NONE, 0), |storage| {
                (
                    storage.slots.as_slice(),
                    storage.first,
                    storage.last,
                    storage.len,
                )
            });
        Iter {
            slots,
            first,
            last,
            remaining,
        }
    }
}

impl Storage {
    fn insert_between(&mut self, node: NodeHandle, previous: usize, next: usize) {
        let index = if self.free == NONE {
            self.slots.len()
        } else {
            let index = self.free;
            self.free = self.slots[index].next;
            index
        };
        node.0.borrow_mut().parent_slot = index;
        let entry = Slot {
            node: Some(node),
            previous,
            next,
        };
        if index == self.slots.len() {
            self.slots.push(entry);
        } else {
            self.slots[index] = entry;
        }
        #[cfg(test)]
        record_sibling_position_work(1);
        if previous == NONE {
            self.first = index;
        } else {
            self.slots[previous].next = index;
            #[cfg(test)]
            record_sibling_position_work(1);
        }
        if next == NONE {
            self.last = index;
        } else {
            self.slots[next].previous = index;
            #[cfg(test)]
            record_sibling_position_work(1);
        }
        self.len += 1;
    }
}

// A local zero-copy view, equivalent in scope to the former slice iterator.
// Only slot IDs move during traversal; no owned state contains this borrow.
struct Iter<'a> {
    slots: &'a [Slot],
    first: usize,
    last: usize,
    remaining: usize,
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a NodeHandle;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let slot = &self.slots[self.first];
        self.first = slot.next;
        self.remaining -= 1;
        Some(slot.node.as_ref().expect("live ordered child"))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl DoubleEndedIterator for Iter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let slot = &self.slots[self.last];
        self.last = slot.previous;
        self.remaining -= 1;
        Some(slot.node.as_ref().expect("live ordered child"))
    }
}

impl ExactSizeIterator for Iter<'_> {}
impl std::iter::FusedIterator for Iter<'_> {}

#[cfg(test)]
pub(super) fn layout_sizes() -> (usize, usize, usize) {
    (
        std::mem::size_of::<Children>(),
        std::mem::size_of::<Storage>(),
        std::mem::size_of::<Slot>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reused_sibling_slots_keep_order_and_release_removed_handles() {
        let mut children = Children::default();
        let a = NodeHandle::text("a");
        let b = NodeHandle::text("b");
        let c = NodeHandle::text("c");
        children.push(a.clone());
        children.push(b.clone());
        children.push(c.clone());
        let removed_slot = b.0.borrow().parent_slot;
        let removed = children.remove(removed_slot);
        assert_eq!(removed, b);
        assert!(children.get_slot(removed_slot).is_none());
        let weak = b.downgrade();
        drop(removed);
        drop(b);
        assert!(!weak.is_alive());
        let d = NodeHandle::text("d");
        children.insert_before(d.clone(), a.0.borrow().parent_slot);
        assert_eq!(d.0.borrow().parent_slot, removed_slot);
        assert_eq!(
            children.iter().cloned().collect::<Vec<_>>(),
            vec![d.clone(), a.clone(), c.clone()]
        );
        assert_eq!(
            children.iter().rev().cloned().collect::<Vec<_>>(),
            vec![c.clone(), a.clone(), d.clone()]
        );
        let mut iter = children.iter();
        assert_eq!(iter.len(), 3);
        assert_eq!(iter.next(), Some(&d));
        assert_eq!(iter.next_back(), Some(&c));
        assert_eq!(iter.len(), 1);
        assert_eq!(iter.next_back(), Some(&a));
        assert!(iter.next().is_none());
        assert!(iter.next_back().is_none());
        drop(iter);
        for node in [d, a, c] {
            children.remove(node.0.borrow().parent_slot);
        }
        assert_eq!(children.len(), 0);
        assert!(children.first().is_none());
        assert!(children.last().is_none());
        assert_eq!(children.iter().len(), 0);
        assert_eq!(children.0.as_ref().unwrap().slots.len(), 3);
    }
}
