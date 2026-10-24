//! Global owner for bounded child-document opacity source caches.

use crate::paint::EffectSourceCache;
use std::collections::HashMap;

// Four per-document 4M-pixel caches cap retained RGBA storage at about 64 MiB.
const MAX_DOCUMENTS: usize = 4;

#[derive(Default)]
pub(super) struct Owner {
    caches: HashMap<usize, EffectSourceCache>,
    oldest_first: Vec<usize>,
}

impl Owner {
    pub(super) fn cache_for(&mut self, document_id: usize) -> &EffectSourceCache {
        if let Some(position) = self.oldest_first.iter().position(|id| *id == document_id) {
            self.oldest_first.remove(position);
        } else {
            while self.caches.len() >= MAX_DOCUMENTS {
                let oldest = self.oldest_first.remove(0);
                self.caches.remove(&oldest);
            }
        }
        self.oldest_first.push(document_id);
        self.caches.entry(document_id).or_default()
    }

    pub(super) fn remove(&mut self, document_id: usize) {
        self.caches.remove(&document_id);
        self.oldest_first.retain(|id| *id != document_id);
    }

    #[cfg(test)]
    pub(super) fn contains(&self, document_id: usize) -> bool {
        self.caches.contains_key(&document_id)
    }

    #[cfg(test)]
    pub(super) fn source_rasters(&self, document_id: usize) -> Option<u64> {
        self.caches
            .get(&document_id)
            .map(EffectSourceCache::source_rasters)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_never_retains_more_than_four_child_documents() {
        let mut owner = Owner::default();
        for document_id in 1..=12 {
            owner.cache_for(document_id);
            assert!(owner.caches.len() <= MAX_DOCUMENTS);
        }
        assert_eq!(owner.oldest_first, [9, 10, 11, 12]);
        assert_eq!(owner.caches.len(), 4);
    }
}
