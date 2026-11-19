//! Opt-in source input for iterating on the child bootstrap in the libtest binary.
//! The returned body is owned by the cache identity; source generation stays
//! pure and both source/parsed caches distinguish different candidate contents.
use super::*;

/// Reads a trusted local candidate only for the three child kinds under study.
/// I/O errors fail the diagnostic instead of falling back to the full source.
pub(super) fn read_source(kind: BootstrapKind) -> Option<Arc<str>> {
    if matches!(kind, BootstrapKind::Window | BootstrapKind::ServiceWorker) {
        return None;
    }
    let path = std::env::var_os("OMOIKANE_TEST_WORKER_BOOTSTRAP_SOURCE")?;
    let body = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read child bootstrap candidate {path:?}: {error}"));
    Some(Arc::from(body))
}
