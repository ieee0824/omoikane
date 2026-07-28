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

    fn with_area<R>(
        &self,
        session_id: u64,
        origin: &StorageOrigin,
        local: bool,
        operation: impl FnOnce(&StorageArea) -> R,
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
        self.with_area(session, origin, local, |area| area.entries.len())
    }

    pub(crate) fn key(
        &self,
        session: u64,
        origin: &StorageOrigin,
        local: bool,
        index: usize,
    ) -> Option<String> {
        self.with_area(session, origin, local, |area| {
            area.entries.get(index).map(|(key, _)| key.clone())
        })
    }

    pub(crate) fn get(
        &self,
        session: u64,
        origin: &StorageOrigin,
        local: bool,
        key: &str,
    ) -> Option<String> {
        self.with_area(session, origin, local, |area| area.get(key))
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
