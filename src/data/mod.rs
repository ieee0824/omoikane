//! Binary data primitives shared by the JavaScript bindings and the resource
//! loading pipeline.
//!
//! The only entry currently living here is the **blob URL store**: the mapping
//! from a `blob:` URL minted by `URL.createObjectURL()` to the bytes and media
//! type it stands for.
//!
//! It is a Rust-side store rather than a JavaScript one because the two
//! consumers run at different times. `fetch()` and `XMLHttpRequest` resolve a
//! `blob:` URL while script is running, so they can read the `Blob` object
//! directly. Layout, on the other hand, resolves `<img src>` and CSS
//! `url(...)` *after* the JavaScript runtime has been dropped (see
//! `crate::paint::render_document_with_url_internal`), with no way to call back
//! into script. Those loads need the bytes to outlive the runtime, so
//! `URL.createObjectURL()` mirrors them here.
//!
//! Entries are therefore a copy of the `Blob`'s bytes, taken when the object URL
//! is created. `Blob` is immutable, so the copy can never go stale. It is freed
//! by `URL.revokeObjectURL()` — or wholesale by [`clear_blob_urls`] when a new
//! global is created for the next Document, which is this engine's unload
//! boundary.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// The bytes and media type a `blob:` URL resolves to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobUrlEntry {
    /// The blob's bytes. Shared rather than cloned because image decoding and
    /// `fetch()` both only read them.
    pub bytes: Rc<Vec<u8>>,
    /// The blob's `type`, already normalized by the JavaScript `Blob`
    /// constructor. Empty when the blob has no type.
    pub media_type: String,
}

thread_local! {
    static BLOB_URLS: RefCell<HashMap<String, BlobUrlEntry>> = RefCell::new(HashMap::new());
}

/// Registers `url` as resolving to `bytes` with media type `media_type`.
///
/// Re-registering the same URL replaces the previous entry. Object URLs are
/// minted with a fresh UUID per call, so in practice this only happens if a page
/// hands the same URL back through a host binding.
///
/// # Examples
///
/// ```
/// use omoikane::data::{lookup_blob_url, register_blob_url};
///
/// register_blob_url(
///     "blob:https://example.test/1".to_string(),
///     b"hello".to_vec(),
///     "text/plain".to_string(),
/// );
/// let entry = lookup_blob_url("blob:https://example.test/1").unwrap();
/// assert_eq!(entry.bytes.as_slice(), b"hello");
/// assert_eq!(entry.media_type, "text/plain");
/// ```
pub fn register_blob_url(url: String, bytes: Vec<u8>, media_type: String) {
    let entry = BlobUrlEntry {
        bytes: Rc::new(bytes),
        media_type,
    };
    BLOB_URLS.with(|store| {
        store.borrow_mut().insert(url, entry);
    });
}

/// Returns the entry `url` resolves to, or `None` when it was never registered
/// or has been revoked.
pub fn lookup_blob_url(url: &str) -> Option<BlobUrlEntry> {
    BLOB_URLS.with(|store| store.borrow().get(url).cloned())
}

/// Removes `url` from the store, returning whether it was registered.
///
/// `URL.revokeObjectURL()` is defined to ignore unknown URLs, so callers use the
/// return value for diagnostics only.
pub fn revoke_blob_url(url: &str) -> bool {
    BLOB_URLS.with(|store| store.borrow_mut().remove(url).is_some())
}

/// Empties the store.
///
/// Called when a new global is created for the next Document: object URLs are
/// scoped to the Document that created them, and nothing can resolve them once
/// it is gone.
pub fn clear_blob_urls() {
    BLOB_URLS.with(|store| store.borrow_mut().clear());
}

/// Returns how many object URLs are currently registered.
pub fn blob_url_count() -> usize {
    BLOB_URLS.with(|store| store.borrow().len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_urls_resolve_to_their_bytes_and_type() {
        clear_blob_urls();
        register_blob_url(
            "blob:https://example.test/a".to_string(),
            vec![1, 2, 3],
            "image/png".to_string(),
        );
        register_blob_url(
            "blob:https://example.test/b".to_string(),
            Vec::new(),
            String::new(),
        );

        let first = lookup_blob_url("blob:https://example.test/a").expect("first entry");
        assert_eq!(first.bytes.as_slice(), &[1, 2, 3]);
        assert_eq!(first.media_type, "image/png");

        let second = lookup_blob_url("blob:https://example.test/b").expect("second entry");
        assert!(second.bytes.is_empty());
        assert_eq!(second.media_type, "");

        assert_eq!(blob_url_count(), 2);
        assert_eq!(lookup_blob_url("blob:https://example.test/missing"), None);
    }

    #[test]
    fn revoking_removes_only_the_named_url() {
        clear_blob_urls();
        register_blob_url("blob:null/a".to_string(), vec![7], String::new());
        register_blob_url("blob:null/b".to_string(), vec![8], String::new());

        assert!(revoke_blob_url("blob:null/a"));
        assert!(!revoke_blob_url("blob:null/a"));
        assert!(!revoke_blob_url("blob:null/never-registered"));

        assert_eq!(lookup_blob_url("blob:null/a"), None);
        assert_eq!(
            lookup_blob_url("blob:null/b").map(|entry| entry.bytes.to_vec()),
            Some(vec![8])
        );
        assert_eq!(blob_url_count(), 1);
    }

    #[test]
    fn re_registering_a_url_replaces_its_entry() {
        clear_blob_urls();
        register_blob_url("blob:null/x".to_string(), vec![1], "text/plain".to_string());
        register_blob_url("blob:null/x".to_string(), vec![2, 3], "text/csv".to_string());

        let entry = lookup_blob_url("blob:null/x").expect("entry");
        assert_eq!(entry.bytes.as_slice(), &[2, 3]);
        assert_eq!(entry.media_type, "text/csv");
        assert_eq!(blob_url_count(), 1);
    }

    #[test]
    fn clearing_drops_every_entry() {
        clear_blob_urls();
        register_blob_url("blob:null/a".to_string(), vec![1], String::new());
        register_blob_url("blob:null/b".to_string(), vec![2], String::new());
        assert_eq!(blob_url_count(), 2);

        clear_blob_urls();

        assert_eq!(blob_url_count(), 0);
        assert_eq!(lookup_blob_url("blob:null/a"), None);
        assert_eq!(lookup_blob_url("blob:null/b"), None);
    }
}
