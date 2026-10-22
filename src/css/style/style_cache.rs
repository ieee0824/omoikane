//! Style map mutation stamps for derived, read-only dependency queries.
use super::ComputedStyle;
use std::cell::RefCell;
use std::collections::{HashMap, hash_map};
use std::ops::{Deref, DerefMut};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

/// The epoch identity follows a map across moves; every mutable borrow advances
/// its revision, including replacements that keep the same keys and length.
#[derive(Debug)]
pub(super) struct StyleCache<K> {
    values: HashMap<K, Arc<ComputedStyle>>,
    epoch: Arc<AtomicUsize>,
}

impl<K> Default for StyleCache<K> {
    fn default() -> Self {
        Self {
            values: HashMap::new(),
            epoch: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl<K> StyleCache<K> {
    fn changed(&self) {
        let revision = self.epoch.load(Ordering::Relaxed);
        self.epoch
            .store(revision.saturating_add(1), Ordering::Relaxed);
    }
}

impl<K> Deref for StyleCache<K> {
    type Target = HashMap<K, Arc<ComputedStyle>>;
    fn deref(&self) -> &Self::Target {
        &self.values
    }
}

impl<K> DerefMut for StyleCache<K> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.changed();
        &mut self.values
    }
}

impl<'a, K> IntoIterator for &'a StyleCache<K> {
    type Item = (&'a K, &'a Arc<ComputedStyle>);
    type IntoIter = hash_map::Iter<'a, K, Arc<ComputedStyle>>;
    fn into_iter(self) -> Self::IntoIter {
        self.values.iter()
    }
}

impl<'a, K> IntoIterator for &'a mut StyleCache<K> {
    type Item = (&'a K, &'a mut Arc<ComputedStyle>);
    type IntoIter = hash_map::IterMut<'a, K, Arc<ComputedStyle>>;
    fn into_iter(self) -> Self::IntoIter {
        self.changed();
        self.values.iter_mut()
    }
}

impl<K> IntoIterator for StyleCache<K> {
    type Item = (K, Arc<ComputedStyle>);
    type IntoIter = hash_map::IntoIter<K, Arc<ComputedStyle>>;
    fn into_iter(self) -> Self::IntoIter {
        self.values.into_iter()
    }
}

#[derive(Debug)]
struct Stamp {
    epoch: Arc<AtomicUsize>,
    revision: usize,
}

impl Stamp {
    fn capture<K>(cache: &StyleCache<K>) -> Option<Self> {
        let revision = cache.epoch.load(Ordering::Relaxed);
        (revision != usize::MAX).then(|| Self {
            epoch: Arc::clone(&cache.epoch),
            revision,
        })
    }
    fn matches<K>(&self, cache: &StyleCache<K>) -> bool {
        Arc::ptr_eq(&self.epoch, &cache.epoch)
            && self.revision == cache.epoch.load(Ordering::Relaxed)
    }
}

#[derive(Debug)]
struct Memo {
    ordinary: Stamp,
    pseudo: Stamp,
    needed: bool,
}

/// Memoizes one derived flag without retaining styles or borrowing their owner.
#[derive(Debug, Default)]
pub(super) struct ContainerContextMemo {
    value: RefCell<Option<Memo>>,
}

impl ContainerContextMemo {
    /// Recomputes after either cache mutates or is replaced. Saturated revisions
    /// are never memoized, so exhaustion cannot hide a later dependency change.
    pub(super) fn get_or_compute<A, B>(
        &self,
        ordinary: &StyleCache<A>,
        pseudo: &StyleCache<B>,
        compute: impl FnOnce() -> bool,
    ) -> bool {
        let hit = self
            .value
            .borrow()
            .as_ref()
            .filter(|memo| memo.ordinary.matches(ordinary) && memo.pseudo.matches(pseudo))
            .map(|memo| memo.needed);
        if let Some(needed) = hit {
            return needed;
        }
        let needed = compute();
        *self.value.borrow_mut() =
            Stamp::capture(ordinary)
                .zip(Stamp::capture(pseudo))
                .map(|(ordinary, pseudo)| Memo {
                    ordinary,
                    pseudo,
                    needed,
                });
        needed
    }
}

#[cfg(test)]
mod tests;
