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
use std::sync::{Arc, Mutex};

/// The bytes and media type a `blob:` URL resolves to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobUrlEntry {
    /// The blob's bytes. Shared rather than cloned because image decoding and
    /// `fetch()` both only read them.
    pub bytes: Arc<Vec<u8>>,
    /// The blob's `type`, already normalized by the JavaScript `Blob`
    /// constructor. Empty when the blob has no type.
    pub media_type: String,
}

/// Owned URL mappings that may be shared explicitly with resource workers.
#[derive(Clone, Default)]
pub(crate) struct BlobUrlStore(Arc<Mutex<HashMap<String, BlobUrlEntry>>>);

thread_local! {
    static BLOB_URLS: RefCell<BlobUrlStore> = RefCell::new(BlobUrlStore::default());
}

/// Captures this thread's current store without copying registered blob bytes.
pub(crate) fn current_blob_url_store() -> BlobUrlStore {
    BLOB_URLS.with(|store| store.borrow().clone())
}

/// Uses an explicitly shared store for a short operation, restoring the prior
/// thread-local store on return or unwind. Revocation and clearing affect all
/// users of the shared handle; unrelated threads retain independent defaults.
pub(crate) fn with_blob_url_store<T>(store: BlobUrlStore, operation: impl FnOnce() -> T) -> T {
    struct Restore(Option<BlobUrlStore>);

    impl Drop for Restore {
        fn drop(&mut self) {
            BLOB_URLS.with(|store| store.replace(self.0.take().unwrap()));
        }
    }

    let _restore = Restore(Some(BLOB_URLS.with(|current| current.replace(store))));
    operation()
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
        bytes: Arc::new(bytes),
        media_type,
    };
    current_blob_url_store()
        .0
        .lock()
        .unwrap()
        .insert(url, entry);
}

/// Returns the entry `url` resolves to, or `None` when it was never registered
/// or has been revoked.
pub fn lookup_blob_url(url: &str) -> Option<BlobUrlEntry> {
    current_blob_url_store().0.lock().unwrap().get(url).cloned()
}

/// Removes `url` from the store, returning whether it was registered.
///
/// `URL.revokeObjectURL()` is defined to ignore unknown URLs, so callers use the
/// return value for diagnostics only.
pub fn revoke_blob_url(url: &str) -> bool {
    current_blob_url_store()
        .0
        .lock()
        .unwrap()
        .remove(url)
        .is_some()
}

/// Empties the store.
///
/// Called when a new global is created for the next Document: object URLs are
/// scoped to the Document that created them, and nothing can resolve them once
/// it is gone.
pub fn clear_blob_urls() {
    current_blob_url_store().0.lock().unwrap().clear();
}

/// Returns how many object URLs are currently registered.
pub fn blob_url_count() -> usize {
    current_blob_url_store().0.lock().unwrap().len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicitly_shared_store_observes_revocation_and_restores_worker_default() {
        clear_blob_urls();
        let revoked = "blob:null/shared-revoked";
        let cleared = "blob:null/shared-cleared";
        register_blob_url(revoked.into(), vec![1, 2, 3], String::new());
        register_blob_url(cleared.into(), vec![4], String::new());
        let original_bytes = lookup_blob_url(revoked).unwrap().bytes;
        let shared = current_blob_url_store();
        let (ready, started) = std::sync::mpsc::channel();
        let (resume, wait) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            // The fresh thread has an independent default store.
            assert_eq!(blob_url_count(), 0);
            register_blob_url(revoked.into(), vec![9], String::new());
            with_blob_url_store(shared, || {
                let entry = lookup_blob_url(revoked).unwrap();
                assert!(Arc::ptr_eq(&entry.bytes, &original_bytes));
                ready.send(()).unwrap();
                wait.recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
                assert!(lookup_blob_url(revoked).is_none());
                assert!(lookup_blob_url(cleared).is_some());
                clear_blob_urls();
            });
            assert_eq!(lookup_blob_url(revoked).unwrap().bytes.as_slice(), &[9]);
            assert_eq!(blob_url_count(), 1);
        });
        started
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(revoke_blob_url(revoked));
        resume.send(()).unwrap();
        worker.join().unwrap();
        assert_eq!(blob_url_count(), 0);
        assert!(lookup_blob_url(cleared).is_none());
    }

    #[test]
    fn nested_store_scopes_restore_after_unwind() {
        clear_blob_urls();
        let url = "blob:null/scoped";
        register_blob_url(url.into(), vec![1], String::new());
        with_blob_url_store(BlobUrlStore::default(), || {
            assert!(lookup_blob_url(url).is_none());
            register_blob_url(url.into(), vec![2], String::new());
            let result = std::panic::catch_unwind(|| {
                with_blob_url_store(BlobUrlStore::default(), || {
                    register_blob_url(url.into(), vec![3], String::new());
                    panic!("exercise scoped restoration");
                });
            });
            assert!(result.is_err());
            assert_eq!(lookup_blob_url(url).unwrap().bytes.as_slice(), &[2]);
        });
        assert_eq!(lookup_blob_url(url).unwrap().bytes.as_slice(), &[1]);
    }

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
        register_blob_url(
            "blob:null/x".to_string(),
            vec![2, 3],
            "text/csv".to_string(),
        );

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
