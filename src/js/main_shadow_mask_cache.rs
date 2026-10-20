//! Main-document ownership for the bounded shadow-mask cache.

use crate::paint::border::FrameShadowMaskCache;

/// Owns masks for exactly one host-selected main Document.
pub(super) struct MainDocumentShadowMaskOwner {
    owner_document_id: usize,
    cache: FrameShadowMaskCache,
}

impl MainDocumentShadowMaskOwner {
    pub(super) fn new(owner_document_id: usize) -> Self {
        Self {
            owner_document_id,
            cache: FrameShadowMaskCache::new(false),
        }
    }

    pub(super) fn cache_for(&mut self, document_id: usize) -> &FrameShadowMaskCache {
        if self.owner_document_id != document_id {
            *self = Self::new(document_id);
        }
        &self.cache
    }

    pub(super) fn clear(&self) {
        self.cache.clear();
    }

    #[cfg(test)]
    pub(super) fn retained_entries(&self) -> usize {
        self.cache.retained_entries()
    }
}
