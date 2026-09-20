use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

const DEFAULT_QUOTA_BYTES: u64 = 50 * 1024 * 1024;
static NEXT_WEB_LOCK_CLIENT_ID: AtomicU64 = AtomicU64::new(1);

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

#[derive(Debug)]
struct StorageState {
    local: HashMap<StorageOrigin, StorageArea>,
    session: HashMap<(u64, StorageOrigin), StorageArea>,
    next_session_id: u64,
    /// Cache Storage namespaces are partitioned by origin, just like
    /// `localStorage`.  Entries retain insertion order because both
    /// `CacheStorage.keys()` and `Cache.keys()` expose deterministic lists.
    caches: HashMap<StorageOrigin, CacheStorageArea>,
    next_cache_entry_id: u64,
    quota_bytes: u64,
    persistence_policy: StoragePersistencePolicy,
    persistence_path: Option<PathBuf>,
    persisted_origins: HashSet<StorageOrigin>,
    web_locks: HashMap<(StorageOrigin, String), WebLockResource>,
    web_lock_requests: HashMap<u64, WebLockRequest>,
    next_web_lock_request_id: u64,
}

impl Default for StorageState {
    fn default() -> Self {
        Self {
            local: HashMap::new(),
            session: HashMap::new(),
            next_session_id: 0,
            caches: HashMap::new(),
            next_cache_entry_id: 0,
            quota_bytes: DEFAULT_QUOTA_BYTES,
            persistence_policy: StoragePersistencePolicy::Unsupported,
            persistence_path: None,
            persisted_origins: HashSet::new(),
            web_locks: HashMap::new(),
            web_lock_requests: HashMap::new(),
            next_web_lock_request_id: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WebLockMode {
    Exclusive,
    Shared,
}

impl WebLockMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Exclusive => "exclusive",
            Self::Shared => "shared",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebLockRequestState {
    Pending,
    Granted,
    Held,
    Stolen,
}

#[derive(Debug)]
struct WebLockRequest {
    origin: StorageOrigin,
    name: String,
    mode: WebLockMode,
    client_id: u64,
    state: WebLockRequestState,
}

#[derive(Debug, Default)]
struct WebLockResource {
    held: Vec<u64>,
    pending: VecDeque<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WebLockRequestResult {
    Granted(u64),
    Pending(u64),
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WebLockStartResult {
    Held,
    Pending,
    Stolen,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WebLockNotificationKind {
    Granted,
    Stolen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WebLockNotification {
    pub(crate) client_id: u64,
    pub(crate) request_id: u64,
    pub(crate) kind: WebLockNotificationKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WebLockSnapshot {
    pub(crate) name: String,
    pub(crate) mode: WebLockMode,
    pub(crate) client_id: u64,
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

/// Host policy used to answer persistent-storage requests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StoragePersistencePolicy {
    /// The embedder has no durable profile store.
    #[default]
    Unsupported,
    /// The embedder supports the API but denies persistence requests.
    Denied,
    /// The embedder allows origins to opt into persistent storage.
    Allow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StorageEstimate {
    pub(crate) usage: u64,
    pub(crate) quota: u64,
}

/// Profile-wide storage shared by all browsing sessions created by an
/// embedder.
#[derive(Debug, Clone, Default)]
pub struct StorageManager(Arc<Mutex<StorageState>>);

impl StorageManager {
    /// Creates an in-memory storage manager with a 50 MiB quota and no
    /// persistent-storage host support.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an in-memory storage manager with host-defined quota and
    /// persistence policy.
    pub fn with_policy(quota_bytes: u64, persistence_policy: StoragePersistencePolicy) -> Self {
        Self(Arc::new(Mutex::new(StorageState {
            quota_bytes,
            persistence_policy,
            ..StorageState::default()
        })))
    }

    /// Creates a storage manager whose granted persistent origins survive a
    /// process restart in `persistence_path`.
    pub fn with_profile(
        persistence_path: impl Into<PathBuf>,
        quota_bytes: u64,
        persistence_policy: StoragePersistencePolicy,
    ) -> io::Result<Self> {
        let persistence_path = persistence_path.into();
        let persisted_origins = load_persisted_origins(&persistence_path)?;
        Ok(Self(Arc::new(Mutex::new(StorageState {
            quota_bytes,
            persistence_policy,
            persistence_path: Some(persistence_path),
            persisted_origins,
            ..StorageState::default()
        }))))
    }

    pub(crate) fn estimate(&self, origin: &StorageOrigin) -> StorageEstimate {
        let state = self.0.lock().expect("storage manager mutex poisoned");
        let local = state.local.get(origin).map_or(0, |area| {
            area.entries.iter().fold(0_u64, |usage, (key, value)| {
                usage
                    .saturating_add(key.len() as u64)
                    .saturating_add(value.len() as u64)
            })
        });
        let caches = state.caches.get(origin).map_or(0, |storage| {
            storage.caches.iter().fold(0_u64, |usage, cache| {
                cache.entries.iter().fold(
                    usage.saturating_add(cache.name.len() as u64),
                    |usage, entry| {
                        usage
                            .saturating_add(entry.request.len() as u64)
                            .saturating_add(entry.response.len() as u64)
                    },
                )
            })
        });
        StorageEstimate {
            usage: local.saturating_add(caches),
            quota: state.quota_bytes,
        }
    }

    pub(crate) fn persisted(&self, origin: &StorageOrigin) -> bool {
        self.0
            .lock()
            .expect("storage manager mutex poisoned")
            .persisted_origins
            .contains(origin)
    }

    pub(crate) fn persistence_policy(&self) -> StoragePersistencePolicy {
        self.0
            .lock()
            .expect("storage manager mutex poisoned")
            .persistence_policy
    }

    pub(crate) fn persist(&self, origin: &StorageOrigin) -> io::Result<bool> {
        let mut state = self.0.lock().expect("storage manager mutex poisoned");
        if state.persistence_policy != StoragePersistencePolicy::Allow {
            return Ok(false);
        }
        if state.persisted_origins.contains(origin) {
            return Ok(true);
        }
        state.persisted_origins.insert(origin.clone());
        if let Some(path) = state.persistence_path.as_deref()
            && let Err(error) = save_persisted_origins(path, &state.persisted_origins)
        {
            state.persisted_origins.remove(origin);
            return Err(error);
        }
        Ok(true)
    }

    /// Allocates an isolated top-level browsing-session identifier.
    pub fn create_session(&self) -> u64 {
        let mut state = self.0.lock().expect("storage manager mutex poisoned");
        state.next_session_id = state.next_session_id.saturating_add(1);
        state.next_session_id
    }

    pub(crate) fn create_web_lock_client(&self) -> u64 {
        NEXT_WEB_LOCK_CLIENT_ID.fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) fn request_web_lock(
        &self,
        origin: &StorageOrigin,
        client_id: u64,
        name: String,
        mode: WebLockMode,
        if_available: bool,
        steal: bool,
    ) -> (WebLockRequestResult, Vec<WebLockNotification>) {
        let mut state = self.0.lock().expect("storage manager mutex poisoned");
        let key = (origin.clone(), name.clone());
        let grantable = web_lock_is_immediately_grantable(&state, &key, mode);
        if if_available && !grantable {
            return (WebLockRequestResult::Unavailable, Vec::new());
        }

        let request_id = state.next_web_lock_request_id;
        state.next_web_lock_request_id = state.next_web_lock_request_id.saturating_add(1);
        state.web_lock_requests.insert(
            request_id,
            WebLockRequest {
                origin: origin.clone(),
                name,
                mode,
                client_id,
                state: WebLockRequestState::Pending,
            },
        );

        let mut notifications = Vec::new();
        if steal {
            let held = state
                .web_locks
                .entry(key.clone())
                .or_default()
                .held
                .drain(..)
                .collect::<Vec<_>>();
            for held_id in held {
                if let Some(held_request) = state.web_lock_requests.get_mut(&held_id) {
                    held_request.state = WebLockRequestState::Stolen;
                    notifications.push(WebLockNotification {
                        client_id: held_request.client_id,
                        request_id: held_id,
                        kind: WebLockNotificationKind::Stolen,
                    });
                }
            }
            state
                .web_locks
                .entry(key)
                .or_default()
                .held
                .push(request_id);
            state
                .web_lock_requests
                .get_mut(&request_id)
                .expect("new Web Lock request must exist")
                .state = WebLockRequestState::Granted;
            return (WebLockRequestResult::Granted(request_id), notifications);
        }

        if grantable {
            state
                .web_locks
                .entry(key)
                .or_default()
                .held
                .push(request_id);
            state
                .web_lock_requests
                .get_mut(&request_id)
                .expect("new Web Lock request must exist")
                .state = WebLockRequestState::Granted;
            (WebLockRequestResult::Granted(request_id), notifications)
        } else {
            state
                .web_locks
                .entry(key)
                .or_default()
                .pending
                .push_back(request_id);
            (WebLockRequestResult::Pending(request_id), notifications)
        }
    }

    pub(crate) fn start_web_lock(&self, request_id: u64) -> WebLockStartResult {
        let mut state = self.0.lock().expect("storage manager mutex poisoned");
        let Some(request) = state.web_lock_requests.get_mut(&request_id) else {
            return WebLockStartResult::Missing;
        };
        match request.state {
            WebLockRequestState::Granted => {
                request.state = WebLockRequestState::Held;
                WebLockStartResult::Held
            }
            WebLockRequestState::Held => WebLockStartResult::Held,
            WebLockRequestState::Pending => WebLockStartResult::Pending,
            WebLockRequestState::Stolen => WebLockStartResult::Stolen,
        }
    }

    pub(crate) fn release_web_lock(&self, request_id: u64) -> Vec<WebLockNotification> {
        let mut state = self.0.lock().expect("storage manager mutex poisoned");
        web_lock_remove_request(&mut state, request_id, true)
    }

    pub(crate) fn cancel_web_lock(&self, request_id: u64) -> (bool, Vec<WebLockNotification>) {
        let mut state = self.0.lock().expect("storage manager mutex poisoned");
        let cancellable = state
            .web_lock_requests
            .get(&request_id)
            .is_some_and(|request| {
                matches!(
                    request.state,
                    WebLockRequestState::Pending | WebLockRequestState::Granted
                )
            });
        if !cancellable {
            return (false, Vec::new());
        }
        (true, web_lock_remove_request(&mut state, request_id, true))
    }

    pub(crate) fn finish_stolen_web_lock(&self, request_id: u64) {
        let mut state = self.0.lock().expect("storage manager mutex poisoned");
        if state
            .web_lock_requests
            .get(&request_id)
            .is_some_and(|request| request.state == WebLockRequestState::Stolen)
        {
            state.web_lock_requests.remove(&request_id);
        }
    }

    pub(crate) fn remove_web_lock_client(&self, client_id: u64) -> Vec<WebLockNotification> {
        let mut state = self.0.lock().expect("storage manager mutex poisoned");
        let request_ids = state
            .web_lock_requests
            .iter()
            .filter_map(|(request_id, request)| {
                (request.client_id == client_id).then_some(*request_id)
            })
            .collect::<Vec<_>>();
        let mut notifications = Vec::new();
        for request_id in request_ids {
            notifications.extend(web_lock_remove_request(&mut state, request_id, true));
        }
        notifications
    }

    pub(crate) fn query_web_locks(
        &self,
        origin: &StorageOrigin,
    ) -> (Vec<WebLockSnapshot>, Vec<WebLockSnapshot>) {
        let state = self.0.lock().expect("storage manager mutex poisoned");
        let mut requests = state.web_lock_requests.iter().collect::<Vec<_>>();
        requests.sort_by_key(|(request_id, _)| **request_id);
        let mut held = Vec::new();
        let mut pending = Vec::new();
        for (_, request) in requests {
            if &request.origin != origin {
                continue;
            }
            let snapshot = WebLockSnapshot {
                name: request.name.clone(),
                mode: request.mode,
                client_id: request.client_id,
            };
            match request.state {
                WebLockRequestState::Granted | WebLockRequestState::Held => held.push(snapshot),
                WebLockRequestState::Pending => pending.push(snapshot),
                WebLockRequestState::Stolen => {}
            }
        }
        (held, pending)
    }

    /// Releases all origin-specific session storage when a tab closes.
    pub(crate) fn remove_session(&self, session_id: u64) {
        let mut state = self.0.lock().expect("storage manager mutex poisoned");
        state
            .session
            .retain(|(session, _), _| *session != session_id);
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
            .map(|storage| {
                storage
                    .caches
                    .iter()
                    .map(|cache| cache.name.clone())
                    .collect()
            })
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
            if !(request_key.0.is_empty() && request_key.1.is_empty()) {
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
        cache.entries.push(CacheEntrySnapshot {
            id,
            request,
            response,
        });
        Some(id)
    }

    pub(crate) fn cache_delete_entry(&self, origin: &StorageOrigin, name: &str, id: u64) -> bool {
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

fn web_lock_is_immediately_grantable(
    state: &StorageState,
    key: &(StorageOrigin, String),
    mode: WebLockMode,
) -> bool {
    let Some(resource) = state.web_locks.get(key) else {
        return true;
    };
    if !resource.pending.is_empty() {
        return false;
    }
    if resource.held.is_empty() {
        return true;
    }
    mode == WebLockMode::Shared
        && resource.held.iter().all(|request_id| {
            state
                .web_lock_requests
                .get(request_id)
                .is_some_and(|request| request.mode == WebLockMode::Shared)
        })
}

fn web_lock_remove_request(
    state: &mut StorageState,
    request_id: u64,
    advance: bool,
) -> Vec<WebLockNotification> {
    let Some(request) = state.web_lock_requests.remove(&request_id) else {
        return Vec::new();
    };
    let key = (request.origin, request.name);
    if let Some(resource) = state.web_locks.get_mut(&key) {
        resource.held.retain(|held| *held != request_id);
        resource.pending.retain(|pending| *pending != request_id);
    }
    if advance {
        web_lock_advance(state, &key)
    } else {
        Vec::new()
    }
}

fn web_lock_advance(
    state: &mut StorageState,
    key: &(StorageOrigin, String),
) -> Vec<WebLockNotification> {
    let mut notifications = Vec::new();
    loop {
        let Some(resource) = state.web_locks.get(key) else {
            break;
        };
        let Some(request_id) = resource.pending.front().copied() else {
            if resource.held.is_empty() {
                state.web_locks.remove(key);
            }
            break;
        };
        let Some(request) = state.web_lock_requests.get(&request_id) else {
            state
                .web_locks
                .get_mut(key)
                .expect("Web Lock resource must exist")
                .pending
                .pop_front();
            continue;
        };
        let can_grant = if resource.held.is_empty() {
            true
        } else {
            request.mode == WebLockMode::Shared
                && resource.held.iter().all(|held_id| {
                    state
                        .web_lock_requests
                        .get(held_id)
                        .is_some_and(|held| held.mode == WebLockMode::Shared)
                })
        };
        if !can_grant {
            break;
        }
        let mode = request.mode;
        let client_id = request.client_id;
        let resource = state
            .web_locks
            .get_mut(key)
            .expect("Web Lock resource must exist");
        resource.pending.pop_front();
        resource.held.push(request_id);
        state
            .web_lock_requests
            .get_mut(&request_id)
            .expect("pending Web Lock request must exist")
            .state = WebLockRequestState::Granted;
        notifications.push(WebLockNotification {
            client_id,
            request_id,
            kind: WebLockNotificationKind::Granted,
        });
        if mode == WebLockMode::Exclusive {
            break;
        }
    }
    notifications
}

fn load_persisted_origins(path: &Path) -> io::Result<HashSet<StorageOrigin>> {
    let encoded = match fs::read(path) {
        Ok(encoded) => encoded,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(error) => return Err(error),
    };
    let values: Vec<String> = serde_json::from_slice(&encoded)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(values
        .into_iter()
        .filter_map(|value| StorageOrigin::from_url(&value))
        .collect())
}

fn save_persisted_origins(path: &Path, origins: &HashSet<StorageOrigin>) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let mut values = origins
        .iter()
        .map(StorageOrigin::serialize)
        .collect::<Vec<_>>();
    values.sort();
    let encoded = serde_json::to_vec_pretty(&values)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, encoded)?;
    fs::rename(temporary, path)
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

    fn temporary_profile(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "omoikane-storage-{name}-{}-{}.json",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ))
    }

    #[test]
    fn closing_a_tab_discards_its_sessions_but_preserves_profile_storage() {
        let manager = StorageManager::new();
        let first = manager.create_session();
        let second = manager.create_session();
        let origin = StorageOrigin::from_url("https://example.test/").unwrap();
        let other_origin = StorageOrigin::from_url("https://other.test/").unwrap();
        for origin in [&origin, &other_origin] {
            manager.set(first, origin, false, "tab".into(), "first".into());
        }
        manager.set(second, &origin, false, "tab".into(), "second".into());
        manager.set(first, &origin, true, "shared".into(), "retained".into());
        manager.remove_session(first);
        assert_eq!(manager.get(first, &origin, false, "tab"), None);
        assert_eq!(manager.get(first, &other_origin, false, "tab"), None);
        assert_eq!(
            manager.get(second, &origin, false, "tab").as_deref(),
            Some("second")
        );
        assert_eq!(
            manager.get(second, &origin, true, "shared").as_deref(),
            Some("retained")
        );
        assert_eq!(manager.0.lock().unwrap().session.len(), 1);
    }

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
        assert_eq!(
            manager.cache_entries(&first, "v1").unwrap()[0].response,
            r#"{"status":201}"#
        );

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

    #[test]
    fn estimate_counts_origin_local_and_cache_bytes() {
        let manager = StorageManager::with_policy(4096, StoragePersistencePolicy::Denied);
        let first = StorageOrigin::from_url("https://example.com/").unwrap();
        let second = StorageOrigin::from_url("https://other.example.com/").unwrap();
        let session = manager.create_session();

        assert_eq!(
            manager.estimate(&first),
            StorageEstimate {
                usage: 0,
                quota: 4096
            }
        );
        manager.set(session, &first, true, "key".into(), "value".into());
        manager.set(
            session,
            &first,
            false,
            "session".into(),
            "not-counted".into(),
        );
        assert_eq!(manager.estimate(&first).usage, 8);
        assert_eq!(manager.estimate(&second).usage, 0);

        manager.cache_open(&first, "v1".into());
        manager
            .cache_put(&first, "v1", "request".into(), "response".into())
            .unwrap();
        assert_eq!(manager.estimate(&first).usage, 8 + 2 + 7 + 8);
        manager.remove(session, &first, true, "key");
        assert_eq!(manager.estimate(&first).usage, 2 + 7 + 8);
    }

    #[test]
    fn persistence_policy_handles_allow_deny_and_unsupported_hosts() {
        let origin = StorageOrigin::from_url("https://example.com/").unwrap();
        for policy in [
            StoragePersistencePolicy::Unsupported,
            StoragePersistencePolicy::Denied,
        ] {
            let manager = StorageManager::with_policy(1024, policy);
            assert!(!manager.persist(&origin).unwrap());
            assert!(!manager.persisted(&origin));
        }

        let manager = StorageManager::with_policy(1024, StoragePersistencePolicy::Allow);
        assert!(manager.persist(&origin).unwrap());
        assert!(manager.persisted(&origin));
    }

    #[test]
    fn persistent_grant_survives_manager_restart_and_remains_origin_scoped() {
        let path = temporary_profile("restart");
        let _ = fs::remove_file(&path);
        let first = StorageOrigin::from_url("https://example.com/").unwrap();
        let second = StorageOrigin::from_url("https://other.example.com/").unwrap();
        {
            let manager =
                StorageManager::with_profile(&path, 2048, StoragePersistencePolicy::Allow).unwrap();
            assert!(manager.persist(&first).unwrap());
        }
        let restarted =
            StorageManager::with_profile(&path, 2048, StoragePersistencePolicy::Allow).unwrap();
        assert!(restarted.persisted(&first));
        assert!(!restarted.persisted(&second));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn failed_profile_write_does_not_report_or_retain_a_grant() {
        let blocker = temporary_profile("blocked-parent");
        let _ = fs::remove_file(&blocker);
        let path = blocker.join("persistent-origins.json");
        let origin = StorageOrigin::from_url("https://example.com/").unwrap();
        let manager =
            StorageManager::with_profile(&path, 1024, StoragePersistencePolicy::Allow).unwrap();
        fs::write(&blocker, b"not a directory").unwrap();

        assert!(manager.persist(&origin).is_err());
        assert!(!manager.persisted(&origin));
        fs::remove_file(blocker).unwrap();
    }
}
