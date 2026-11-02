//! Owned media-segment parsing boundary for Media Source Extensions.

use super::*;
use std::sync::Arc;

/// Metadata produced after an embedder accepts one media segment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MediaSourceSegment {
    /// Presentation duration contributed by the accepted segment.
    pub duration_seconds: f64,
    /// Number of owned bytes consumed by the provider.
    pub byte_length: usize,
}

/// Error returned by a media-segment provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaSourceError {
    UnsupportedType,
    Decode,
    QuotaExceeded,
}

/// Embedder boundary for demuxing owned Media Source segment bytes.
pub trait MediaSourceProvider: Send + Sync {
    /// Returns whether this provider accepts the supplied MIME type.
    fn supports_type(&self, mime_type: &str) -> bool;
    /// Consumes an owned segment and returns its presentation metadata.
    fn append_segment(
        &self,
        mime_type: &str,
        bytes: Vec<u8>,
    ) -> Result<MediaSourceSegment, MediaSourceError>;
}

/// Deterministic provider used by headless runtimes and tests.
#[derive(Clone, Debug)]
pub struct DeterministicMediaSourceProvider {
    quota_bytes: usize,
}

impl Default for DeterministicMediaSourceProvider {
    fn default() -> Self {
        Self {
            quota_bytes: 16 * 1024 * 1024,
        }
    }
}

impl DeterministicMediaSourceProvider {
    /// Creates a deterministic provider with the supplied per-segment quota.
    pub fn new(quota_bytes: usize) -> Self {
        Self { quota_bytes }
    }
}

impl MediaSourceProvider for DeterministicMediaSourceProvider {
    fn supports_type(&self, mime_type: &str) -> bool {
        matches!(
            mime_type.split(';').next().unwrap_or_default().trim(),
            "audio/mp4" | "audio/webm" | "video/mp4" | "video/webm"
        )
    }

    fn append_segment(
        &self,
        mime_type: &str,
        bytes: Vec<u8>,
    ) -> Result<MediaSourceSegment, MediaSourceError> {
        if !self.supports_type(mime_type) {
            return Err(MediaSourceError::UnsupportedType);
        }
        if bytes.len() > self.quota_bytes {
            return Err(MediaSourceError::QuotaExceeded);
        }
        if bytes.is_empty() {
            return Err(MediaSourceError::Decode);
        }
        Ok(MediaSourceSegment {
            duration_seconds: (bytes.len() as f64 / 1000.0).max(0.001),
            byte_length: bytes.len(),
        })
    }
}

pub(super) struct State {
    provider: Arc<dyn MediaSourceProvider>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            provider: Arc::new(DeterministicMediaSourceProvider::default()),
        }
    }
}

fn buffer_source_bytes(value: Option<&JsValue>, context: &mut Context) -> JsResult<Vec<u8>> {
    let object = value
        .and_then(JsValue::as_object)
        .ok_or_else(|| JsNativeError::typ().with_message("appendBuffer requires a BufferSource"))?;
    let view = JsTypedArray::from_object(object.clone())
        .map_err(|_| JsNativeError::typ().with_message("appendBuffer requires a BufferSource"))?;
    let offset = view.byte_offset(context)?;
    let length = view.byte_length(context)?;
    let buffer = view
        .buffer(context)?
        .as_object()
        .and_then(|value| JsArrayBuffer::from_object(value.clone()).ok())
        .ok_or_else(|| JsNativeError::typ().with_message("BufferSource has no ArrayBuffer"))?;
    let data = buffer
        .data()
        .ok_or_else(|| JsNativeError::typ().with_message("BufferSource is detached"))?;
    data.get(offset..)
        .and_then(|tail| tail.get(..length))
        .map(<[u8]>::to_vec)
        .ok_or_else(|| {
            JsNativeError::typ()
                .with_message("BufferSource is out of bounds")
                .into()
        })
}

fn operation_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let operation = string_argument(args.first(), "", context)?;
    let mime_type = string_argument(args.get(1), "", context)?;
    with_host_state(|host| {
        let host = host.borrow();
        let provider = &host.media_source.provider;
        if operation == "supports" {
            return Ok(JsValue::from(provider.supports_type(&mime_type)));
        }
        if operation != "append" {
            return Err(JsNativeError::typ()
                .with_message("Unknown MediaSource operation")
                .into());
        }
        let bytes = buffer_source_bytes(args.get(2), context)?;
        let result = provider
            .append_segment(&mime_type, bytes)
            .map_err(|error| {
                let (name, message) = match error {
                    MediaSourceError::UnsupportedType => {
                        ("NotSupportedError", "Unsupported media type")
                    }
                    MediaSourceError::Decode => {
                        ("EncodingError", "Media segment could not be decoded")
                    }
                    MediaSourceError::QuotaExceeded => {
                        ("QuotaExceededError", "Media segment quota exceeded")
                    }
                };
                JsNativeError::error().with_message(format!("{name}:{message}"))
            })?;
        Ok(JsValue::from(JsString::from(
            serde_json::json!({
                "duration": result.duration_seconds,
                "byteLength": result.byte_length,
            })
            .to_string(),
        )))
    })
}

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_media_source"),
        3,
        NativeFunction::from_copy_closure(operation_native),
    )
}

impl JsRuntime {
    /// Replaces the demux provider used by subsequent SourceBuffer appends.
    pub fn set_media_source_provider(&mut self, provider: Arc<dyn MediaSourceProvider>) {
        self.host_state.borrow_mut().media_source.provider = provider;
    }
}
