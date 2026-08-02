//! Browser-chrome primitives shared by native frontends.
//!
//! A CdpSession owns one browsing context. A platform window normally owns
//! more than one of those contexts (tabs), and it also needs state which is
//! not part of a document: the selected tab, download records, and browser
//! notifications. This module keeps that state deterministic and
//! toolkit-independent.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;

use serde_json::json;

use crate::cdp::{CdpSession, JsonRpcError};
use crate::frame::{BrowserFrame, FrameError, render_browser_frame};

/// Stable identity for a tab in one PlatformBrowser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TabId(u64);

impl TabId {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Stable identity for a download in one PlatformBrowser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DownloadId(u64);

impl DownloadId {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// User-visible metadata for a tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabInfo {
    pub id: TabId,
    pub url: String,
    pub title: String,
    pub active: bool,
}

/// The terminal state of a browser download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadState {
    InProgress { loaded: u64, total: Option<u64> },
    Completed,
    Failed(String),
    Cancelled,
}

/// A download record retained by the browser chrome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadInfo {
    pub id: DownloadId,
    pub url: String,
    pub filename: String,
    pub mime_type: Option<String>,
    pub bytes: Vec<u8>,
    pub state: DownloadState,
}

/// Notifications emitted by tab and download operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserEvent {
    TabOpened(TabInfo),
    TabActivated(TabId),
    TabNavigated(TabInfo),
    TabClosed(TabId),
    DownloadStarted(DownloadInfo),
    DownloadProgress {
        id: DownloadId,
        loaded: u64,
        total: Option<u64>,
    },
    DownloadFinished(DownloadInfo),
    DownloadFailed { id: DownloadId, error: String },
    DownloadCancelled(DownloadId),
}

/// Errors returned by browser-chrome operations.
#[derive(Debug)]
pub enum BrowserError {
    InvalidTab(TabId),
    NoActiveTab,
    InvalidDownload(DownloadId),
    DownloadAlreadyFinished(DownloadId),
    IdExhausted(&'static str),
    Navigation(JsonRpcError),
    Frame(FrameError),
    Network(String),
    DownloadFailed { id: DownloadId, error: String },
}

impl fmt::Display for BrowserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTab(id) => write!(f, "unknown tab {}", id.get()),
            Self::NoActiveTab => f.write_str("no active tab"),
            Self::InvalidDownload(id) => write!(f, "unknown download {}", id.get()),
            Self::DownloadAlreadyFinished(id) => {
                write!(f, "download {} is already finished", id.get())
            }
            Self::IdExhausted(kind) => write!(f, "{kind} id space exhausted"),
            Self::Navigation(error) => write!(f, "navigation failed: {}", error.message),
            Self::Frame(error) => write!(f, "frame failed: {error}"),
            Self::Network(error) => write!(f, "download failed: {error}"),
            Self::DownloadFailed { id, error } => {
                write!(f, "download {} failed: {error}", id.get())
            }
        }
    }
}

impl std::error::Error for BrowserError {}

impl From<FrameError> for BrowserError {
    fn from(error: FrameError) -> Self {
        Self::Frame(error)
    }
}

#[derive(Debug)]
struct BrowserTab {
    info: TabInfo,
    session: CdpSession,
}

/// A deterministic tab strip, navigation coordinator, and download store.
///
/// The type owns no native window objects. Frontends can map TabId to their
/// own window/widget handles while all browser state stays on the engine side.
/// All operations are synchronous, matching the existing CdpSession API.
#[derive(Debug)]
pub struct PlatformBrowser {
    tabs: BTreeMap<TabId, BrowserTab>,
    active_tab: Option<TabId>,
    next_tab_id: u64,
    downloads: BTreeMap<DownloadId, DownloadInfo>,
    next_download_id: u64,
    events: VecDeque<BrowserEvent>,
}

impl PlatformBrowser {
    /// Creates an empty browser. Use open_tab to create the first context.
    pub fn new() -> Self {
        Self {
            tabs: BTreeMap::new(),
            active_tab: None,
            next_tab_id: 1,
            downloads: BTreeMap::new(),
            next_download_id: 1,
            events: VecDeque::new(),
        }
    }

    /// Creates a browser with one active tab.
    pub fn with_tab(url: Option<&str>) -> Result<Self, BrowserError> {
        let mut browser = Self::new();
        browser.open_tab(url)?;
        Ok(browser)
    }

    /// Opens a new tab and makes it active. None or an empty URL creates
    /// about:blank.
    pub fn open_tab(&mut self, url: Option<&str>) -> Result<TabId, BrowserError> {
        let id = TabId(self.next_tab_id);
        self.next_tab_id = self
            .next_tab_id
            .checked_add(1)
            .ok_or(BrowserError::IdExhausted("tab"))?;
        let mut session = CdpSession::new().map_err(|message| {
            BrowserError::Network(format!("failed to create tab runtime: {message}"))
        })?;
        let url = url.filter(|url| !url.trim().is_empty());
        if let Some(url) = url {
            session
                .dispatch("Page.navigate", json!({ "url": url }))
                .map_err(BrowserError::Navigation)?;
        }
        let info = TabInfo {
            id,
            url: session.current_url().to_string(),
            title: document_title(&mut session),
            active: true,
        };
        if let Some(previous) = self.active_tab
            && let Some(tab) = self.tabs.get_mut(&previous)
        {
            tab.info.active = false;
        }
        self.active_tab = Some(id);
        self.tabs.insert(id, BrowserTab { info: info.clone(), session });
        self.events.push_back(BrowserEvent::TabOpened(info));
        Ok(id)
    }

    pub fn active_tab(&self) -> Option<TabId> {
        self.active_tab
    }

    pub fn tab_info(&self, id: TabId) -> Option<&TabInfo> {
        self.tabs.get(&id).map(|tab| &tab.info)
    }

    pub fn tabs(&self) -> impl Iterator<Item = &TabInfo> {
        self.tabs.values().map(|tab| &tab.info)
    }

    /// Selects a tab without changing its document or history.
    pub fn activate_tab(&mut self, id: TabId) -> Result<(), BrowserError> {
        if !self.tabs.contains_key(&id) {
            return Err(BrowserError::InvalidTab(id));
        }
        if self.active_tab == Some(id) {
            return Ok(());
        }
        if let Some(previous) = self.active_tab
            && let Some(tab) = self.tabs.get_mut(&previous)
        {
            tab.info.active = false;
        }
        self.tabs
            .get_mut(&id)
            .expect("tab was checked above")
            .info
            .active = true;
        self.active_tab = Some(id);
        self.events.push_back(BrowserEvent::TabActivated(id));
        Ok(())
    }

    /// Closes a tab. The next tab to the right is selected, falling back to
    /// the tab on the left. Closing the last tab leaves no active context.
    pub fn close_tab(&mut self, id: TabId) -> Result<(), BrowserError> {
        if !self.tabs.contains_key(&id) {
            return Err(BrowserError::InvalidTab(id));
        }
        let was_active = self.active_tab == Some(id);
        self.tabs.remove(&id);
        self.events.push_back(BrowserEvent::TabClosed(id));
        if was_active {
            let next = self
                .tabs
                .range((std::ops::Bound::Excluded(id), std::ops::Bound::Unbounded))
                .next()
                .map(|(id, _)| *id)
                .or_else(|| self.tabs.keys().next_back().copied());
            self.active_tab = None;
            if let Some(next) = next {
                self.activate_tab(next)?;
            }
        }
        Ok(())
    }

    pub fn active_session_mut(&mut self) -> Result<&mut CdpSession, BrowserError> {
        let id = self.active_tab.ok_or(BrowserError::NoActiveTab)?;
        self.tabs
            .get_mut(&id)
            .map(|tab| &mut tab.session)
            .ok_or(BrowserError::InvalidTab(id))
    }

    /// Navigates a tab and refreshes its title/address-bar metadata.
    pub fn navigate(&mut self, id: TabId, url: &str) -> Result<&TabInfo, BrowserError> {
        let tab = self.tabs.get_mut(&id).ok_or(BrowserError::InvalidTab(id))?;
        tab.session
            .dispatch("Page.navigate", json!({ "url": url }))
            .map_err(BrowserError::Navigation)?;
        refresh_tab_info(tab);
        self.events.push_back(BrowserEvent::TabNavigated(tab.info.clone()));
        Ok(&tab.info)
    }

    /// Reloads the active tab, preserving its current history entry.
    pub fn reload(&mut self) -> Result<&TabInfo, BrowserError> {
        let id = self.active_tab.ok_or(BrowserError::NoActiveTab)?;
        let tab = self.tabs.get_mut(&id).expect("active tab exists");
        tab.session
            .dispatch("Page.reload", json!({}))
            .map_err(BrowserError::Navigation)?;
        refresh_tab_info(tab);
        self.events.push_back(BrowserEvent::TabNavigated(tab.info.clone()));
        Ok(&tab.info)
    }

    /// Traverses session history using the page's standard History API.
    pub fn traverse_history(&mut self, delta: i32) -> Result<&TabInfo, BrowserError> {
        let id = self.active_tab.ok_or(BrowserError::NoActiveTab)?;
        let tab = self.tabs.get_mut(&id).expect("active tab exists");
        if delta == 0 {
            return Ok(&tab.info);
        }
        tab.session
            .dispatch(
                "Runtime.evaluate",
                json!({ "expression": format!("history.go({delta})") }),
            )
            .map_err(BrowserError::Navigation)?;
        refresh_tab_info(tab);
        self.events.push_back(BrowserEvent::TabNavigated(tab.info.clone()));
        Ok(&tab.info)
    }

    pub fn render_active(
        &mut self,
        width: u32,
        height: u32,
        elapsed_ms: u64,
    ) -> Result<BrowserFrame, BrowserError> {
        let session = self.active_session_mut()?;
        render_browser_frame(session, width, height, elapsed_ms).map_err(Into::into)
    }

    /// Downloads a URL using the selected tab's cookie-aware HTTP client.
    /// The response is retained in memory for a frontend-provided sink.
    pub fn download(
        &mut self,
        url: &str,
        suggested_filename: Option<&str>,
    ) -> Result<DownloadId, BrowserError> {
        let tab_id = self.active_tab.ok_or(BrowserError::NoActiveTab)?;
        let tab = self.tabs.get_mut(&tab_id).expect("active tab exists");
        let id = DownloadId(self.next_download_id);
        self.next_download_id = self
            .next_download_id
            .checked_add(1)
            .ok_or(BrowserError::IdExhausted("download"))?;
        let filename = suggested_filename
            .map(sanitize_filename)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| filename_from_url(url));
        let mut info = DownloadInfo {
            id,
            url: url.to_string(),
            filename,
            mime_type: None,
            bytes: Vec::new(),
            state: DownloadState::InProgress {
                loaded: 0,
                total: None,
            },
        };
        self.events.push_back(BrowserEvent::DownloadStarted(info.clone()));
        self.downloads.insert(id, info.clone());

        let response = match tab.session.http_client_mut().get(url) {
            Ok(response) => response,
            Err(error) => {
                let message = error.to_string();
                info.state = DownloadState::Failed(message.clone());
                self.downloads.insert(id, info);
                self.events.push_back(BrowserEvent::DownloadFailed { id, error: message.clone() });
                return Err(BrowserError::DownloadFailed { id, error: message });
            }
        };
        info.mime_type = response.header("content-type").map(ToOwned::to_owned);
        if suggested_filename.is_none()
            && let Some(disposition) = response.header("content-disposition")
            && let Some(name) = content_disposition_filename(disposition)
        {
            info.filename = name;
        }
        let total = response
            .header("content-length")
            .and_then(|value| value.parse::<u64>().ok());
        info.state = DownloadState::InProgress {
            loaded: response.body().len() as u64,
            total,
        };
        self.downloads.insert(id, info.clone());
        self.events.push_back(BrowserEvent::DownloadProgress {
            id,
            loaded: response.body().len() as u64,
            total,
        });
        info.bytes = response.body().to_vec();
        info.state = DownloadState::Completed;
        self.downloads.insert(id, info.clone());
        self.events.push_back(BrowserEvent::DownloadFinished(info));
        Ok(id)
    }

    pub fn download_info(&self, id: DownloadId) -> Option<&DownloadInfo> {
        self.downloads.get(&id)
    }

    /// Cancels a download which has not reached a terminal state.
    pub fn cancel_download(&mut self, id: DownloadId) -> Result<(), BrowserError> {
        let info = self
            .downloads
            .get_mut(&id)
            .ok_or(BrowserError::InvalidDownload(id))?;
        if !matches!(info.state, DownloadState::InProgress { .. }) {
            return Err(BrowserError::DownloadAlreadyFinished(id));
        }
        info.state = DownloadState::Cancelled;
        info.bytes.clear();
        self.events.push_back(BrowserEvent::DownloadCancelled(id));
        Ok(())
    }

    pub fn drain_events(&mut self) -> Vec<BrowserEvent> {
        self.events.drain(..).collect()
    }
}

impl Default for PlatformBrowser {
    fn default() -> Self {
        Self::new()
    }
}

fn refresh_tab_info(tab: &mut BrowserTab) {
    tab.info.url = tab.session.current_url().to_string();
    tab.info.title = document_title(&mut tab.session);
}

fn document_title(session: &mut CdpSession) -> String {
    session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": "document.title", "returnByValue": true }),
        )
        .ok()
        .and_then(|value| value["result"]["value"].as_str().map(ToOwned::to_owned))
        .filter(|title| !title.is_empty())
        .unwrap_or_default()
}

fn sanitize_filename(value: &str) -> String {
    let mut filename = value
        .chars()
        .map(|character| {
            if character.is_control() || matches!(character, '/' | '\\' | ':') {
                '_'
            } else {
                character
            }
        })
        .collect::<String>();
    while filename.starts_with('.') {
        filename.remove(0);
    }
    filename.trim().to_string()
}

fn filename_from_url(url: &str) -> String {
    let without_fragment = url.split_once('#').map_or(url, |(path, _)| path);
    let path = without_fragment
        .split_once('?')
        .map_or(without_fragment, |(path, _)| path);
    let candidate = path.rsplit('/').next().unwrap_or_default();
    let candidate = sanitize_filename(candidate);
    if candidate.is_empty() {
        "download".to_string()
    } else {
        candidate
    }
}

fn content_disposition_filename(value: &str) -> Option<String> {
    value
        .split(';')
        .skip(1)
        .filter_map(|part| part.trim().split_once('='))
        .find_map(|(name, value)| {
            if name.trim().eq_ignore_ascii_case("filename") {
                Some(sanitize_filename(value.trim().trim_matches('"')))
            } else {
                None
            }
        })
        .filter(|name| !name.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn navigate_url(html: &str) -> String {
        let encoded = html
            .bytes()
            .map(|byte| format!("%{byte:02X}"))
            .collect::<String>();
        format!("data:text/html,{encoded}")
    }

    #[test]
    fn tab_strip_selects_adjacent_tab_when_active_tab_closes() {
        let mut browser = PlatformBrowser::new();
        let first = browser.open_tab(Some(&navigate_url("<title>one</title>"))).unwrap();
        let second = browser.open_tab(Some(&navigate_url("<title>two</title>"))).unwrap();
        let third = browser.open_tab(Some(&navigate_url("<title>three</title>"))).unwrap();
        assert_eq!(browser.active_tab(), Some(third));
        assert_eq!(browser.tab_info(first).unwrap().title, "one");

        browser.close_tab(third).unwrap();
        assert_eq!(browser.active_tab(), Some(second));
        browser.close_tab(second).unwrap();
        assert_eq!(browser.active_tab(), Some(first));
        browser.close_tab(first).unwrap();
        assert_eq!(browser.active_tab(), None);
    }

    #[test]
    fn navigation_refreshes_address_and_title_without_recreating_tab() {
        let mut browser = PlatformBrowser::new();
        let tab = browser.open_tab(Some(&navigate_url("<title>before</title>"))).unwrap();
        browser
            .navigate(tab, &navigate_url("<title>after</title>"))
            .unwrap();
        let info = browser.tab_info(tab).unwrap();
        assert_eq!(info.title, "after");
        assert!(info.url.starts_with("data:text/html,"));
        assert_eq!(browser.tabs().count(), 1);
    }

    #[test]
    fn download_uses_content_disposition_and_emits_ordered_lifecycle() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Type: text/plain\r\nContent-Disposition: attachment; filename=report.txt\r\n\r\nhello",
                )
                .unwrap();
        });
        let mut browser = PlatformBrowser::with_tab(None).unwrap();
        let id = browser
            .download(&format!("http://{address}/download"), None)
            .unwrap();
        server.join().unwrap();
        let download = browser.download_info(id).unwrap();
        assert_eq!(download.filename, "report.txt");
        assert_eq!(download.bytes, b"hello");
        assert_eq!(download.state, DownloadState::Completed);
        let events = browser.drain_events();
        assert!(matches!(events[0], BrowserEvent::TabOpened(_)));
        assert!(matches!(events[1], BrowserEvent::DownloadStarted(_)));
        assert!(matches!(events[2], BrowserEvent::DownloadProgress { .. }));
        assert!(matches!(events[3], BrowserEvent::DownloadFinished(_)));
    }

    #[test]
    fn filename_is_safe_and_has_a_fallback() {
        assert_eq!(sanitize_filename("../a/b.txt"), "_a_b.txt");
        assert_eq!(filename_from_url("https://example.test/path/?q=1"), "download");
        assert_eq!(filename_from_url("https://example.test/file.txt#part"), "file.txt");
    }
}
