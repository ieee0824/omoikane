//! Deterministic media-capture provider state exposed through private bindings.

use super::*;
use std::sync::Arc;

/// Kind of a capture device supplied by the embedder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaDeviceKind {
    AudioInput,
    VideoInput,
    AudioOutput,
}

impl MediaDeviceKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::AudioInput => "audioinput",
            Self::VideoInput => "videoinput",
            Self::AudioOutput => "audiooutput",
        }
    }
}

/// Owned device description installed at the media-capture provider boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaCaptureDevice {
    pub kind: MediaDeviceKind,
    pub device_id: String,
    pub group_id: String,
    pub label: String,
}

/// Immutable capture-provider result consumed at one DOM API boundary.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MediaCaptureSnapshot {
    pub permission_granted: bool,
    pub devices: Vec<MediaCaptureDevice>,
}

/// Embedder contract for native or deterministic media-capture backends.
///
/// Implementations return owned snapshots, so DOM state never retains a
/// borrow into a platform adapter and a single API operation observes one
/// coherent permission/device generation.
pub trait MediaCaptureProvider: Send + Sync {
    fn snapshot(&self) -> MediaCaptureSnapshot;
}

/// Mutable-at-construction provider used by tests and headless embedders.
#[derive(Clone, Debug, Default)]
pub struct StaticMediaCaptureProvider {
    snapshot: MediaCaptureSnapshot,
}

impl StaticMediaCaptureProvider {
    /// Creates a provider with an explicit permission and device snapshot.
    pub fn new(permission_granted: bool, devices: Vec<MediaCaptureDevice>) -> Self {
        Self {
            snapshot: MediaCaptureSnapshot {
                permission_granted,
                devices,
            },
        }
    }
}

impl MediaCaptureProvider for StaticMediaCaptureProvider {
    fn snapshot(&self) -> MediaCaptureSnapshot {
        self.snapshot.clone()
    }
}

impl MediaCaptureDevice {
    /// Creates a deterministic capture device.
    pub fn new(
        kind: MediaDeviceKind,
        device_id: impl Into<String>,
        label: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            device_id: device_id.into(),
            group_id: String::new(),
            label: label.into(),
        }
    }
}

pub(super) struct State {
    provider: Arc<dyn MediaCaptureProvider>,
}

impl State {
    pub(super) fn set_permission(&mut self, granted: bool) {
        let devices = self.provider.snapshot().devices;
        self.provider = Arc::new(StaticMediaCaptureProvider::new(granted, devices));
    }
}

impl Default for State {
    fn default() -> Self {
        Self {
            provider: Arc::new(StaticMediaCaptureProvider::default()),
        }
    }
}

fn snapshot_native(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, document_id)?;
    with_host_state(|state| {
        let state = state.borrow();
        let snapshot = state.media_capture.provider.snapshot();
        let devices = snapshot
            .devices
            .iter()
            .map(|device| {
                serde_json::json!({
                    "kind": device.kind.as_str(),
                    "deviceId": device.device_id,
                    "groupId": device.group_id,
                    "label": if snapshot.permission_granted { &device.label } else { "" },
                })
            })
            .collect::<Vec<_>>();
        let snapshot = serde_json::json!({
            "permission": snapshot.permission_granted,
            "devices": devices,
        })
        .to_string();
        Ok(JsValue::from(JsString::from(snapshot.as_str())))
    })
}

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_media_capture_snapshot"),
        1,
        NativeFunction::from_copy_closure(snapshot_native),
    )
}

impl JsRuntime {
    /// Installs the provider used by subsequent media-capture operations.
    pub fn set_media_capture_provider(&mut self, provider: Arc<dyn MediaCaptureProvider>) {
        self.host_state.borrow_mut().media_capture.provider = provider;
    }

    /// Replaces the deterministic list returned by `enumerateDevices()`.
    pub fn set_media_capture_devices(&mut self, devices: Vec<MediaCaptureDevice>) {
        let permission = self
            .host_state
            .borrow()
            .media_capture
            .provider
            .snapshot()
            .permission_granted;
        self.set_media_capture_provider(Arc::new(StaticMediaCaptureProvider::new(
            permission, devices,
        )));
    }

    /// Sets the deterministic camera/microphone permission decision.
    pub fn set_media_capture_permission(&mut self, granted: bool) {
        self.host_state
            .borrow_mut()
            .media_capture
            .set_permission(granted);
        self.notify_permission_change("camera", if granted { "granted" } else { "denied" });
        self.notify_permission_change("microphone", if granted { "granted" } else { "denied" });
    }
}
