//! Owned provider boundary for WebCodecs codec operations.

use super::*;
use serde::Deserialize;
use std::sync::Arc;

/// Codec pipeline selected by a WebCodecs object.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum WebCodecKind {
    /// Encoded video to raw video frames.
    VideoDecoder,
    /// Raw video frames to encoded video chunks.
    VideoEncoder,
    /// Encoded audio to raw audio data.
    AudioDecoder,
    /// Raw audio data to encoded audio chunks.
    AudioEncoder,
    /// Encoded image data to raw video frames.
    ImageDecoder,
}

/// Owned configuration passed to a codec provider.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WebCodecConfig {
    /// Pipeline being configured.
    pub kind: WebCodecKind,
    /// Codec registry string supplied by the page.
    pub codec: String,
    /// Coded width when configuring video.
    #[serde(default)]
    pub coded_width: Option<u32>,
    /// Coded height when configuring video.
    #[serde(default)]
    pub coded_height: Option<u32>,
    /// Audio sample rate when configuring audio.
    #[serde(default)]
    pub sample_rate: Option<u32>,
    /// Audio channel count when configuring audio.
    #[serde(default)]
    pub number_of_channels: Option<u32>,
}

/// Owned packet submitted to a configured provider.
#[derive(Clone, Debug, PartialEq)]
pub struct WebCodecPacket {
    /// Pipeline that owns the packet.
    pub kind: WebCodecKind,
    /// Configured codec registry string.
    pub codec: String,
    /// Presentation timestamp in microseconds.
    pub timestamp: i64,
    /// Optional presentation duration in microseconds.
    pub duration: Option<u64>,
    /// Whether an encoded video packet is independently decodable.
    pub key_frame: bool,
    /// Bytes copied from the page at the native boundary.
    pub bytes: Vec<u8>,
}

/// Metadata returned after a provider accepts one packet.
#[derive(Clone, Debug, PartialEq)]
pub struct WebCodecOutput {
    /// Output presentation timestamp in microseconds.
    pub timestamp: i64,
    /// Optional output duration in microseconds.
    pub duration: Option<u64>,
    /// Owned output bytes.
    pub bytes: Vec<u8>,
}

/// Failure returned by a WebCodecs provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebCodecError {
    /// The provider does not implement the requested configuration.
    Unsupported,
    /// The submitted packet could not be processed.
    Data,
}

/// Embedder boundary for codec capability queries and owned packet processing.
pub trait WebCodecsProvider: Send + Sync {
    /// Returns whether this provider can instantiate the exact configuration.
    fn supports(&self, config: &WebCodecConfig) -> bool;
    /// Processes one owned packet under an already-supported configuration.
    fn process(
        &self,
        config: &WebCodecConfig,
        packet: WebCodecPacket,
    ) -> Result<WebCodecOutput, WebCodecError>;
}

/// Deterministic provider that accepts only explicitly listed codec strings.
#[derive(Clone, Debug, Default)]
pub struct DeterministicWebCodecsProvider {
    supported_codecs: Vec<String>,
}

impl DeterministicWebCodecsProvider {
    /// Creates a deterministic provider accepting the listed registry strings.
    pub fn new(codecs: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            supported_codecs: codecs.into_iter().map(Into::into).collect(),
        }
    }
}

impl WebCodecsProvider for DeterministicWebCodecsProvider {
    fn supports(&self, config: &WebCodecConfig) -> bool {
        self.supported_codecs
            .iter()
            .any(|codec| codec == &config.codec)
    }

    fn process(
        &self,
        config: &WebCodecConfig,
        packet: WebCodecPacket,
    ) -> Result<WebCodecOutput, WebCodecError> {
        if !self.supports(config) {
            return Err(WebCodecError::Unsupported);
        }
        if packet.codec != config.codec || packet.kind != config.kind {
            return Err(WebCodecError::Data);
        }
        Ok(WebCodecOutput {
            timestamp: packet.timestamp,
            duration: packet.duration,
            bytes: packet.bytes,
        })
    }
}

pub(super) struct State {
    provider: Arc<dyn WebCodecsProvider>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            provider: Arc::new(DeterministicWebCodecsProvider::default()),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PacketMetadata {
    kind: WebCodecKind,
    codec: String,
    timestamp: i64,
    #[serde(default)]
    duration: Option<u64>,
    #[serde(default)]
    key_frame: bool,
}

fn parse_json<T: for<'de> Deserialize<'de>>(
    value: Option<&JsValue>,
    context: &mut Context,
    label: &str,
) -> JsResult<T> {
    let text = string_argument(value, "", context)?;
    serde_json::from_str(&text).map_err(|_| {
        JsNativeError::typ()
            .with_message(format!("Invalid {label}"))
            .into()
    })
}

fn buffer_source_bytes(value: Option<&JsValue>, context: &mut Context) -> JsResult<Vec<u8>> {
    let object = value.and_then(JsValue::as_object).ok_or_else(|| {
        JsNativeError::typ().with_message("WebCodecs input requires a BufferSource")
    })?;
    let view = JsTypedArray::from_object(object.clone()).map_err(|_| {
        JsNativeError::typ().with_message("WebCodecs input requires a BufferSource")
    })?;
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
    data.get(offset..offset.saturating_add(length))
        .map(<[u8]>::to_vec)
        .ok_or_else(|| {
            JsNativeError::typ()
                .with_message("BufferSource is out of bounds")
                .into()
        })
}

fn operation_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let operation = string_argument(args.first(), "", context)?;
    let config: WebCodecConfig = parse_json(args.get(1), context, "WebCodecs config")?;
    with_host_state(|host| {
        let host = host.borrow();
        if operation == "supports" {
            return Ok(JsValue::from(host.web_codecs.provider.supports(&config)));
        }
        if operation != "process" {
            return Err(JsNativeError::typ()
                .with_message("Unknown WebCodecs operation")
                .into());
        }
        let metadata: PacketMetadata = parse_json(args.get(2), context, "WebCodecs packet")?;
        let bytes = buffer_source_bytes(args.get(3), context)?;
        let output = host
            .web_codecs
            .provider
            .process(
                &config,
                WebCodecPacket {
                    kind: metadata.kind,
                    codec: metadata.codec,
                    timestamp: metadata.timestamp,
                    duration: metadata.duration,
                    key_frame: metadata.key_frame,
                    bytes,
                },
            )
            .map_err(|error| match error {
                WebCodecError::Unsupported => JsNativeError::error()
                    .with_message("NotSupportedError: Unsupported codec configuration"),
                WebCodecError::Data => {
                    JsNativeError::error().with_message("DataError: Codec rejected input")
                }
            })?;
        Ok(JsValue::from(JsString::from(
            serde_json::json!({
                "timestamp": output.timestamp,
                "duration": output.duration,
                "data": output.bytes,
            })
            .to_string(),
        )))
    })
}

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_web_codecs"),
        4,
        NativeFunction::from_copy_closure(operation_native),
    )
}

impl JsRuntime {
    /// Replaces the provider used by subsequent WebCodecs operations.
    pub fn set_web_codecs_provider(&mut self, provider: Arc<dyn WebCodecsProvider>) {
        self.host_state.borrow_mut().web_codecs.provider = provider;
    }
}
