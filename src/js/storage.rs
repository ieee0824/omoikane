use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct StorageOrigin {
    scheme: String,
    host: String,
    port: u16,
}

impl StorageOrigin {
    pub(crate) fn from_url(url: &str) -> Option<Self> {
        let parsed = url.parse::<crate::http::Url>().ok()?;
        Some(Self {
            scheme: parsed.scheme().to_ascii_lowercase(),
            host: parsed.host().to_ascii_lowercase(),
            port: parsed.port(),
        })
    }

    pub(crate) fn serialize(&self) -> String {
        format!("{}://{}:{}", self.scheme, self.host, self.port)
    }
}

#[derive(Debug, Default)]
struct StorageArea {
    entries: Vec<(String, String)>,
}

impl StorageArea {
    fn get(&self, key: &str) -> Option<String> {
        self.entries
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
    }

    fn set(&mut self, key: String, value: String) -> Option<String> {
        if let Some((_, current)) = self.entries.iter_mut().find(|(name, _)| name == &key) {
            return Some(std::mem::replace(current, value));
        }
        self.entries.push((key, value));
        None
    }

    fn remove(&mut self, key: &str) -> Option<String> {
        let index = self.entries.iter().position(|(name, _)| name == key)?;
        Some(self.entries.remove(index).1)
    }
}

#[derive(Debug, Default)]
struct StorageState {
    local: HashMap<StorageOrigin, StorageArea>,
    session: HashMap<(u64, StorageOrigin), StorageArea>,
    next_session_id: u64,
    /// Cache Storage namespaces are partitioned by origin, just like
    /// `localStorage`.  Entries retain insertion order because both
    /// `CacheStorage.keys()` and `Cache.keys()` expose deterministic lists.
    caches: HashMap<StorageOrigin, CacheStorageArea>,
    next_cache_entry_id: u64,
}

#[derive(Debug, Default)]
struct CacheStorageArea {
    caches: Vec<CacheNamespace>,
}

#[derive(Debug)]
struct CacheNamespace {
    name: String,
    entries: Vec<CacheEntrySnapshot>,
}

/// A host-owned Cache entry.  The JavaScript layer owns the Web IDL objects;
/// the native side only retains JSON snapshots so no Boa value can leak
/// between realms.  `id` is stable for the lifetime of the entry and lets the
/// JS matching implementation delete exactly the records it selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CacheEntrySnapshot {
    pub(crate) id: u64,
    pub(crate) request: String,
    pub(crate) response: String,
}

#[derive(Debug, Clone, Default)]
pub struct StorageManager(Arc<Mutex<StorageState>>);

impl StorageManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocates an isolated top-level browsing-session identifier.
    pub fn create_session(&self) -> u64 {
        let mut state = self.0.lock().expect("storage manager mutex poisoned");
        state.next_session_id = state.next_session_id.saturating_add(1);
        state.next_session_id
    }

    pub(crate) fn cache_open(&self, origin: &StorageOrigin, name: String) -> bool {
        let mut state = self.0.lock().expect("storage manager mutex poisoned");
        let storage = state.caches.entry(origin.clone()).or_default();
        if storage.caches.iter().any(|cache| cache.name == name) {
            return false;
        }
        storage.caches.push(CacheNamespace {
            name,
            entries: Vec::new(),
        });
        true
    }

    pub(crate) fn cache_has(&self, origin: &StorageOrigin, name: &str) -> bool {
        let state = self.0.lock().expect("storage manager mutex poisoned");
        state
            .caches
            .get(origin)
            .is_some_and(|storage| storage.caches.iter().any(|cache| cache.name == name))
    }

    pub(crate) fn cache_names(&self, origin: &StorageOrigin) -> Vec<String> {
        let state = self.0.lock().expect("storage manager mutex poisoned");
        state
            .caches
            .get(origin)
            .map(|storage| storage.caches.iter().map(|cache| cache.name.clone()).collect())
            .unwrap_or_default()
    }

    pub(crate) fn cache_delete(&self, origin: &StorageOrigin, name: &str) -> bool {
        let mut state = self.0.lock().expect("storage manager mutex poisoned");
        let Some(storage) = state.caches.get_mut(origin) else {
            return false;
        };
        let Some(index) = storage.caches.iter().position(|cache| cache.name == name) else {
            return false;
        };
        storage.caches.remove(index);
        true
    }

    pub(crate) fn cache_entries(
        &self,
        origin: &StorageOrigin,
        name: &str,
    ) -> Option<Vec<CacheEntrySnapshot>> {
        let state = self.0.lock().expect("storage manager mutex poisoned");
        let storage = state.caches.get(origin)?;
        storage
            .caches
            .iter()
            .find(|cache| cache.name == name)
            .map(|cache| cache.entries.clone())
    }

    /// Stores one request/response pair.  Cache.put replaces an existing
    /// entry with the same request URL and method; preserving the original
    /// insertion position avoids surprising `keys()` reorderings.
    pub(crate) fn cache_put(
        &self,
        origin: &StorageOrigin,
        name: &str,
        request: String,
        response: String,
    ) -> Option<u64> {
        let mut state = self.0.lock().expect("storage manager mutex poisoned");
        let request_key = cache_request_replacement_key(&request);
        {
            let storage = state.caches.get_mut(origin)?;
            let cache = storage.caches.iter_mut().find(|cache| cache.name == name)?;
            // A malformed snapshot has no stable replacement key.  Do not
            // let the empty sentinel collide with another malformed entry.
            if !request_key.0.is_empty() {
                if let Some(existing) = cache
                    .entries
                    .iter_mut()
                    .find(|entry| cache_request_replacement_key(&entry.request) == request_key)
                {
                    existing.request = request;
                    existing.response = response;
                    return Some(existing.id);
                }
            }
        }

        state.next_cache_entry_id = state.next_cache_entry_id.saturating_add(1);
        let id = state.next_cache_entry_id;
        let storage = state.caches.get_mut(origin)?;
        let cache = storage.caches.iter_mut().find(|cache| cache.name == name)?;
        cache.entries.push(CacheEntrySnapshot { id, request, response });
        Some(id)
    }

    pub(crate) fn cache_delete_entry(
        &self,
        origin: &StorageOrigin,
        name: &str,
        id: u64,
    ) -> bool {
        let mut state = self.0.lock().expect("storage manager mutex poisoned");
        let Some(storage) = state.caches.get_mut(origin) else {
            return false;
        };
        let Some(cache) = storage.caches.iter_mut().find(|cache| cache.name == name) else {
            return false;
        };
        let Some(index) = cache.entries.iter().position(|entry| entry.id == id) else {
            return false;
        };
        cache.entries.remove(index);
        true
    }

    fn with_area<R>(
        &self,
        session_id: u64,
        origin: &StorageOrigin,
        local: bool,
        operation: impl FnOnce(Option<&StorageArea>) -> R,
    ) -> R {
        let state = self.0.lock().expect("storage manager mutex poisoned");
        let area = if local {
            state.local.get(origin)
        } else {
            state.session.get(&(session_id, origin.clone()))
        };
        operation(area)
    }

    fn with_area_mut<R>(
        &self,
        session_id: u64,
        origin: &StorageOrigin,
        local: bool,
        operation: impl FnOnce(&mut StorageArea) -> R,
    ) -> R {
        let mut state = self.0.lock().expect("storage manager mutex poisoned");
        let area = if local {
            state.local.entry(origin.clone()).or_default()
        } else {
            state
                .session
                .entry((session_id, origin.clone()))
                .or_default()
        };
        operation(area)
    }

    pub(crate) fn length(&self, session: u64, origin: &StorageOrigin, local: bool) -> usize {
        self.with_area(session, origin, local, |area| {
            area.map_or(0, |area| area.entries.len())
        })
    }

    pub(crate) fn key(
        &self,
        session: u64,
        origin: &StorageOrigin,
        local: bool,
        index: usize,
    ) -> Option<String> {
        self.with_area(session, origin, local, |area| {
            area.and_then(|area| area.entries.get(index).map(|(key, _)| key.clone()))
        })
    }

    pub(crate) fn get(
        &self,
        session: u64,
        origin: &StorageOrigin,
        local: bool,
        key: &str,
    ) -> Option<String> {
        self.with_area(session, origin, local, |area| {
            area.and_then(|area| area.get(key))
        })
    }

    pub(crate) fn set(
        &self,
        session: u64,
        origin: &StorageOrigin,
        local: bool,
        key: String,
        value: String,
    ) -> Option<String> {
        self.with_area_mut(session, origin, local, |area| area.set(key, value))
    }

    pub(crate) fn remove(
        &self,
        session: u64,
        origin: &StorageOrigin,
        local: bool,
        key: &str,
    ) -> Option<String> {
        self.with_area_mut(session, origin, local, |area| area.remove(key))
    }

    pub(crate) fn clear(&self, session: u64, origin: &StorageOrigin, local: bool) -> bool {
        self.with_area_mut(session, origin, local, |area| {
            let changed = !area.entries.is_empty();
            area.entries.clear();
            changed
        })
    }
}

/// Extracts the URL/method portion used by Cache.put's replacement rule.
/// Request snapshots are intentionally JSON objects, but malformed payloads
/// should never make the storage mutex panic; an empty key simply means the
/// record cannot replace another one.
fn cache_request_replacement_key(request: &str) -> (String, String) {
    serde_json::from_str::<serde_json::Value>(request)
        .ok()
        .and_then(|value| {
            let url = value.get("url")?.as_str()?;
            let canonical_url = url
                .split_once('#')
                .map_or_else(|| url.to_string(), |(url, _)| url.to_string());
            Some((
                canonical_url,
                value.get("method")?.as_str()?.to_ascii_uppercase(),
            ))
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_an_absent_area_does_not_allocate_it() {
        let manager = StorageManager::new();
        let session = manager.create_session();
        let origin = StorageOrigin::from_url("https://example.com/").unwrap();

        assert_eq!(manager.length(session, &origin, true), 0);
        assert_eq!(manager.key(session, &origin, false, 0), None);
        assert_eq!(manager.get(session, &origin, true, "missing"), None);

        let state = manager.0.lock().unwrap();
        assert!(state.local.is_empty());
        assert!(state.session.is_empty());
    }

    #[test]
    fn cache_namespaces_are_origin_partitioned_and_put_replaces_in_place() {
        let manager = StorageManager::new();
        let first = StorageOrigin::from_url("https://example.com/").unwrap();
        let second = StorageOrigin::from_url("https://other.example.com/").unwrap();

        assert!(manager.cache_open(&first, "v1".to_string()));
        assert!(!manager.cache_open(&first, "v1".to_string()));
        assert_eq!(manager.cache_names(&first), vec!["v1".to_string()]);
        assert!(manager.cache_names(&second).is_empty());

        let first_id = manager
            .cache_put(
                &first,
                "v1",
                r#"{"url":"https://example.com/a","method":"GET"}"#.to_string(),
                r#"{"status":200}"#.to_string(),
            )
            .unwrap();
        let replacement_id = manager
            .cache_put(
                &first,
                "v1",
                r#"{"url":"https://example.com/a","method":"GET"}"#.to_string(),
                r#"{"status":201}"#.to_string(),
            )
            .unwrap();
        assert_eq!(first_id, replacement_id);
        assert_eq!(manager.cache_entries(&first, "v1").unwrap().len(), 1);
        assert_eq!(manager.cache_entries(&first, "v1").unwrap()[0].response, r#"{"status":201}"#);

        let malformed_id = manager
            .cache_put(&first, "v1", "not-json".to_string(), "{}".to_string())
            .unwrap();
        let second_malformed_id = manager
            .cache_put(&first, "v1", "still-not-json".to_string(), "{}".to_string())
            .unwrap();
        assert_ne!(malformed_id, second_malformed_id);
        assert_eq!(manager.cache_entries(&first, "v1").unwrap().len(), 3);

        assert!(manager.cache_delete(&first, "v1"));
        assert!(!manager.cache_has(&first, "v1"));
    }
}
