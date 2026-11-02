//! Host-provider boundary for the Media Session API.

use super::*;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// One immutable artwork candidate from `MediaMetadata`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaSessionArtwork {
    /// Absolute artwork URL.
    pub src: String,
    /// Declared image sizes.
    pub sizes: String,
    /// Declared MIME type.
    pub media_type: String,
}

/// Result of fetching one artwork candidate through the document fetch path.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaSessionArtworkFetch {
    /// Absolute artwork URL requested by the host.
    pub url: String,
    /// Owned response bytes on success.
    pub bytes: Option<Vec<u8>>,
    /// Script-visible failure text on failure.
    pub error: Option<String>,
}

/// Metadata exposed to the host media controls.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaSessionMetadata {
    /// Track title.
    pub title: String,
    /// Track artist.
    pub artist: String,
    /// Album title.
    pub album: String,
    /// Immutable artwork candidates.
    pub artwork: Vec<MediaSessionArtwork>,
}

/// Current playback position reported to the host.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaSessionPositionState {
    /// Media duration in seconds.
    pub duration: f64,
    /// Current position in seconds.
    pub position: f64,
    /// Current non-zero playback rate.
    pub playback_rate: f64,
}

/// Complete, owned Media Session state published to an embedder.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaSessionSnapshot {
    /// Metadata, or `None` after it is cleared.
    pub metadata: Option<MediaSessionMetadata>,
    /// `none`, `paused`, or `playing`.
    pub playback_state: String,
    /// Position state, or `None` after it is cleared.
    pub position_state: Option<MediaSessionPositionState>,
    /// Actions with installed handlers.
    pub handled_actions: Vec<String>,
}

impl Default for MediaSessionSnapshot {
    fn default() -> Self {
        Self {
            metadata: None,
            playback_state: "none".to_string(),
            position_state: None,
            handled_actions: Vec::new(),
        }
    }
}

/// Embedder boundary for Media Session state and action capabilities.
pub trait MediaSessionProvider: Send + Sync {
    /// Publishes one internally validated, owned state snapshot.
    fn publish(&self, snapshot: MediaSessionSnapshot);

    /// Returns whether the host supports installing the supplied action.
    fn supports_action(&self, action: &str) -> bool;

    /// Receives an artwork fetch result produced by the document fetch/CSP path.
    fn artwork_fetched(&self, result: MediaSessionArtworkFetch);
}

/// Deterministic provider used by headless runtimes and tests.
#[derive(Debug, Default)]
pub struct DeterministicMediaSessionProvider {
    snapshot: Mutex<MediaSessionSnapshot>,
    artwork_fetches: Mutex<Vec<MediaSessionArtworkFetch>>,
}

impl DeterministicMediaSessionProvider {
    /// Returns the latest published snapshot.
    pub fn snapshot(&self) -> MediaSessionSnapshot {
        self.snapshot.lock().unwrap().clone()
    }

    /// Returns all artwork fetch results in completion order.
    pub fn artwork_fetches(&self) -> Vec<MediaSessionArtworkFetch> {
        self.artwork_fetches.lock().unwrap().clone()
    }
}

impl MediaSessionProvider for DeterministicMediaSessionProvider {
    fn publish(&self, snapshot: MediaSessionSnapshot) {
        *self.snapshot.lock().unwrap() = snapshot;
    }

    fn supports_action(&self, action: &str) -> bool {
        matches!(
            action,
            "play"
                | "pause"
                | "previoustrack"
                | "nexttrack"
                | "seekbackward"
                | "seekforward"
                | "stop"
                | "seekto"
                | "skipad"
                | "togglemicrophone"
                | "togglecamera"
                | "togglescreenshare"
                | "hangup"
                | "previousslide"
                | "nextslide"
                | "enterpictureinpicture"
                | "voiceactivity"
        )
    }

    fn artwork_fetched(&self, result: MediaSessionArtworkFetch) {
        self.artwork_fetches.lock().unwrap().push(result);
    }
}

pub(super) struct State {
    provider: Arc<dyn MediaSessionProvider>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            provider: Arc::new(DeterministicMediaSessionProvider::default()),
        }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        self.provider.publish(MediaSessionSnapshot::default());
    }
}

fn typed_array_bytes(value: Option<&JsValue>, context: &mut Context) -> JsResult<Vec<u8>> {
    let object = value
        .and_then(JsValue::as_object)
        .ok_or_else(|| JsNativeError::typ().with_message("Artwork result requires bytes"))?;
    let view = JsTypedArray::from_object(object.clone())
        .map_err(|_| JsNativeError::typ().with_message("Artwork result requires bytes"))?;
    let offset = view.byte_offset(context)?;
    let length = view.byte_length(context)?;
    let buffer = view
        .buffer(context)?
        .as_object()
        .and_then(|value| JsArrayBuffer::from_object(value.clone()).ok())
        .ok_or_else(|| JsNativeError::typ().with_message("Artwork bytes have no ArrayBuffer"))?;
    let data = buffer
        .data()
        .ok_or_else(|| JsNativeError::typ().with_message("Artwork bytes are detached"))?;
    data.get(offset..)
        .and_then(|tail| tail.get(..length))
        .map(<[u8]>::to_vec)
        .ok_or_else(|| {
            JsNativeError::typ()
                .with_message("Artwork bytes are detached")
                .into()
        })
}

fn operation_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let operation = string_argument(args.first(), "", context)?;
    let value = string_argument(args.get(1), "", context)?;
    with_host_state(|host| {
        let host = host.borrow();
        match operation.as_str() {
            "supports" => Ok(JsValue::from(
                host.media_session.provider.supports_action(&value),
            )),
            "publish" => {
                let snapshot: MediaSessionSnapshot =
                    serde_json::from_str(&value).map_err(|_| {
                        JsNativeError::typ().with_message("Invalid Media Session snapshot")
                    })?;
                host.media_session.provider.publish(snapshot);
                Ok(JsValue::undefined())
            }
            "artwork-success" => {
                let bytes = typed_array_bytes(args.get(2), context)?;
                host.media_session
                    .provider
                    .artwork_fetched(MediaSessionArtworkFetch {
                        url: value,
                        bytes: Some(bytes),
                        error: None,
                    });
                Ok(JsValue::undefined())
            }
            "artwork-error" => {
                let error = string_argument(args.get(2), "Artwork fetch failed", context)?;
                host.media_session
                    .provider
                    .artwork_fetched(MediaSessionArtworkFetch {
                        url: value,
                        bytes: None,
                        error: Some(error),
                    });
                Ok(JsValue::undefined())
            }
            _ => Err(JsNativeError::typ()
                .with_message("Unknown Media Session operation")
                .into()),
        }
    })
}

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_media_session"),
        3,
        NativeFunction::from_copy_closure(operation_native),
    )
}

impl JsRuntime {
    /// Replaces the provider that receives subsequent Media Session snapshots.
    pub fn set_media_session_provider(&mut self, provider: Arc<dyn MediaSessionProvider>) {
        self.host_state.borrow_mut().media_session.provider = provider;
    }

    /// Dispatches one host Media Session action to the currently installed handler.
    pub fn dispatch_media_session_action(
        &mut self,
        action: &str,
        details: serde_json::Value,
    ) -> JsResult<bool> {
        if !self
            .host_state
            .borrow()
            .media_session
            .provider
            .supports_action(action)
        {
            return Err(JsNativeError::typ()
                .with_message("Unsupported Media Session action")
                .into());
        }
        let source = format!(
            "__omoikane_dispatch_media_session_action({},{})",
            serde_json::to_string(action).unwrap(),
            serde_json::to_string(&details).unwrap()
        );
        Ok(self.eval(&source)?.to_boolean())
    }

    /// Fetches one current artwork candidate through the document's fetch and CSP path.
    pub fn request_media_session_artwork(&mut self, index: usize) -> JsResult<()> {
        self.eval(&format!("__omoikane_fetch_media_session_artwork({index})"))?;
        Ok(())
    }
}
