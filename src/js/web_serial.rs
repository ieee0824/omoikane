//! Host-owned port, permission, signal, and byte I/O boundary for Web Serial.

use super::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

/// Stable host description of a serial port.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SerialPortInfo {
    /// Embedder-only stable identifier, never exposed to script.
    pub id: String,
    /// USB vendor identifier when the port is USB-backed.
    pub usb_vendor_id: Option<u16>,
    /// USB product identifier when the port is USB-backed.
    pub usb_product_id: Option<u16>,
    /// Bluetooth service UUID when the port is RFCOMM-backed.
    pub bluetooth_service_class_id: Option<String>,
    /// Whether the physical or virtual device is currently connected.
    pub connected: bool,
}

/// Normalized chooser filter.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SerialPortFilter {
    /// USB vendor identifier to match.
    pub usb_vendor_id: Option<u16>,
    /// USB product identifier to match; requires `usb_vendor_id`.
    pub usb_product_id: Option<u16>,
    /// Bluetooth RFCOMM service UUID to match.
    pub bluetooth_service_class_id: Option<String>,
}

/// Validated connection settings.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SerialOptions {
    /// Requested baud rate.
    pub baud_rate: u32,
    /// Number of data bits per frame.
    pub data_bits: u8,
    /// Number of stop bits per frame.
    pub stop_bits: u8,
    /// Parity mode (`none`, `even`, or `odd`).
    pub parity: String,
    /// Read buffer capacity in bytes.
    pub buffer_size: u32,
    /// Flow-control mode (`none` or `hardware`).
    pub flow_control: String,
}

/// Requested control-line changes. `None` preserves the current output.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SerialOutputSignals {
    /// Data Terminal Ready change; `None` preserves the current output.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_signal_change"
    )]
    pub data_terminal_ready: Option<bool>,
    /// Request To Send change; `None` preserves the current output.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_signal_change"
    )]
    pub request_to_send: Option<bool>,
    /// Break change; `None` preserves the current output.
    #[serde(
        rename = "break",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_signal_change"
    )]
    pub break_signal: Option<bool>,
}

fn deserialize_signal_change<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<bool>, D::Error> {
    bool::deserialize(deserializer).map(Some)
}

impl SerialOutputSignals {
    fn is_empty(&self) -> bool {
        self.data_terminal_ready.is_none()
            && self.request_to_send.is_none()
            && self.break_signal.is_none()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct SerialOutputState {
    data_terminal_ready: bool,
    request_to_send: bool,
    break_signal: bool,
}

impl SerialOutputState {
    fn updated(self, changes: &SerialOutputSignals) -> Self {
        Self {
            data_terminal_ready: changes
                .data_terminal_ready
                .unwrap_or(self.data_terminal_ready),
            request_to_send: changes.request_to_send.unwrap_or(self.request_to_send),
            break_signal: changes.break_signal.unwrap_or(self.break_signal),
        }
    }
}

/// Device control-line input values.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SerialInputSignals {
    /// Data Carrier Detect input.
    pub data_carrier_detect: bool,
    /// Clear To Send input.
    pub clear_to_send: bool,
    /// Ring Indicator input.
    pub ring_indicator: bool,
    /// Data Set Ready input.
    pub data_set_ready: bool,
}

/// Failure reported by a serial backend.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SerialError {
    /// DOMException-compatible error name.
    pub name: String,
    /// Human-readable failure detail.
    pub message: String,
}

/// Provider-issued identity of one successful open operation.
///
/// Providers must never reuse an identity within their lifetime. Each operation
/// must validate the identity atomically with its port state update. The host
/// retains the issuing provider along with this identity.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SerialSessionId(u64);

impl SerialSessionId {
    /// Constructs an identity supplied by an embedder's non-reusing allocator.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
}

/// Embedder contract for serial enumeration, permission, and byte I/O.
///
/// Implementations must bind every successful open to a fresh session identity.
/// Session checks and I/O mutations must share the same synchronization boundary;
/// checking a token before a later origin/port operation is insufficient.
/// See `docs/web-serial-provider.md` for the session API migration.
pub trait SerialProvider: Send + Sync {
    /// Lists ports permitted for this origin.
    fn get_ports(&self, origin: &str) -> Vec<SerialPortInfo>;
    /// Selects a connected port and grants origin-scoped permission.
    fn request_port(
        &self,
        origin: &str,
        filters: &[SerialPortFilter],
        allowed_bluetooth_services: &[String],
    ) -> Result<SerialPortInfo, SerialError>;
    /// Opens a permitted port and returns a non-reused provider identity.
    fn open(
        &self,
        origin: &str,
        port: &str,
        options: &SerialOptions,
    ) -> Result<SerialSessionId, SerialError>;
    /// Closes only the matching session, succeeding without I/O if it expired.
    /// Operational failure leaves its logical ownership intact for retry.
    fn close(&self, origin: &str, port: &str, session: SerialSessionId) -> Result<(), SerialError>;
    /// Reads bytes only while this exact session remains open.
    fn read(
        &self,
        origin: &str,
        port: &str,
        session: SerialSessionId,
    ) -> Result<Option<Vec<u8>>, SerialError>;
    /// Writes bytes only while this exact session remains open.
    fn write(
        &self,
        origin: &str,
        port: &str,
        session: SerialSessionId,
        bytes: &[u8],
    ) -> Result<(), SerialError>;
    /// Reads control lines for the matching open session.
    fn get_signals(
        &self,
        origin: &str,
        port: &str,
        session: SerialSessionId,
    ) -> Result<SerialInputSignals, SerialError>;
    /// Changes control lines for the matching open session.
    fn set_signals(
        &self,
        origin: &str,
        port: &str,
        session: SerialSessionId,
        signals: &SerialOutputSignals,
    ) -> Result<(), SerialError>;
    /// Revokes origin permission when no session for that origin remains open.
    fn forget(&self, origin: &str, port: &str) -> Result<(), SerialError>;
    /// Invalidates this exact logical session when its owner retires.
    ///
    /// Token validation and invalidation must be atomic with port operations.
    /// Stale tokens must leave newer sessions untouched. Invalidation cannot
    /// fail; providers own physical cleanup and must not reuse an unsafe device
    /// handle before cleanup completes. Later opens may report NetworkError.
    fn retire(&self, origin: &str, port: &str, session: SerialSessionId);
}

#[derive(Debug)]
struct DeterministicPort {
    info: SerialPortInfo,
    permitted_origins: HashSet<String>,
    open_origins: HashMap<String, SerialSessionId>,
    reads: VecDeque<Vec<u8>>,
    writes: Vec<Vec<u8>>,
    input_signals: SerialInputSignals,
    output_signals: SerialOutputState,
}

/// Deterministic in-memory serial backend for headless embedders and tests.
#[derive(Debug, Default)]
pub struct DeterministicSerialProvider {
    ports: Mutex<HashMap<String, DeterministicPort>>,
    next_error: Mutex<Option<SerialError>>,
    next_session: std::sync::atomic::AtomicU64,
}

impl DeterministicSerialProvider {
    /// Adds or replaces an emulated port.
    pub fn add_port(&self, info: SerialPortInfo) {
        self.ports.lock().unwrap().insert(
            info.id.clone(),
            DeterministicPort {
                info,
                permitted_origins: HashSet::new(),
                open_origins: HashMap::new(),
                reads: VecDeque::new(),
                writes: Vec::new(),
                input_signals: SerialInputSignals::default(),
                output_signals: SerialOutputState::default(),
            },
        );
    }

    /// Changes the emulated physical connection state.
    pub fn set_connected(&self, id: &str, connected: bool) {
        if let Some(port) = self.ports.lock().unwrap().get_mut(id) {
            port.info.connected = connected;
            if !connected {
                port.open_origins.clear();
            }
        }
    }

    /// Queues bytes returned by the next read.
    pub fn enqueue_read(&self, id: &str, bytes: Vec<u8>) {
        if let Some(port) = self.ports.lock().unwrap().get_mut(id) {
            port.reads.push_back(bytes);
        }
    }

    /// Returns all byte chunks written to a port.
    pub fn writes(&self, id: &str) -> Vec<Vec<u8>> {
        self.ports
            .lock()
            .unwrap()
            .get(id)
            .map(|port| port.writes.clone())
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(super) fn output_signals_for_test(&self, id: &str) -> (bool, bool, bool) {
        let ports = self.ports.lock().unwrap();
        let value = &ports.get(id).unwrap().output_signals;
        (
            value.data_terminal_ready,
            value.request_to_send,
            value.break_signal,
        )
    }

    /// Makes the next fallible provider operation return `error`.
    pub fn fail_next(&self, error: SerialError) {
        *self.next_error.lock().unwrap() = Some(error);
    }

    fn take_error(&self) -> Result<(), SerialError> {
        match self.next_error.lock().unwrap().take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

fn matches_filter(info: &SerialPortInfo, filter: &SerialPortFilter) -> bool {
    filter
        .usb_vendor_id
        .is_none_or(|id| info.usb_vendor_id == Some(id))
        && filter
            .usb_product_id
            .is_none_or(|id| info.usb_product_id == Some(id))
        && filter
            .bluetooth_service_class_id
            .as_ref()
            .is_none_or(|id| info.bluetooth_service_class_id.as_ref() == Some(id))
}

impl SerialProvider for DeterministicSerialProvider {
    fn get_ports(&self, origin: &str) -> Vec<SerialPortInfo> {
        self.ports
            .lock()
            .unwrap()
            .values()
            .filter(|port| port.permitted_origins.contains(origin))
            .map(|port| port.info.clone())
            .collect()
    }

    fn request_port(
        &self,
        origin: &str,
        filters: &[SerialPortFilter],
        _allowed_bluetooth_services: &[String],
    ) -> Result<SerialPortInfo, SerialError> {
        self.take_error()?;
        let mut ports = self.ports.lock().unwrap();
        let port = ports
            .values_mut()
            .find(|port| {
                port.info.connected
                    && (filters.is_empty()
                        || filters
                            .iter()
                            .any(|filter| matches_filter(&port.info, filter)))
            })
            .ok_or_else(|| SerialError {
                name: "NotFoundError".into(),
                message: "No serial port was selected".into(),
            })?;
        port.permitted_origins.insert(origin.to_owned());
        Ok(port.info.clone())
    }

    fn open(
        &self,
        origin: &str,
        port: &str,
        _options: &SerialOptions,
    ) -> Result<SerialSessionId, SerialError> {
        self.take_error()?;
        let mut ports = self.ports.lock().unwrap();
        let port = ports.get_mut(port).ok_or_else(not_found)?;
        if !port.info.connected || !port.permitted_origins.contains(origin) {
            return Err(not_found());
        }
        if port.open_origins.contains_key(origin) {
            return Err(SerialError {
                name: "InvalidStateError".into(),
                message: "The serial port is already open".into(),
            });
        }
        let id = self
            .next_session
            .fetch_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |value| value.checked_add(1),
            )
            .map_err(|_| SerialError {
                name: "OperationError".into(),
                message: "Serial session identities exhausted".into(),
            })?;
        let session = SerialSessionId::new(id);
        port.open_origins.insert(origin.to_owned(), session);
        Ok(session)
    }

    fn close(&self, origin: &str, port: &str, session: SerialSessionId) -> Result<(), SerialError> {
        let mut ports = self.ports.lock().unwrap();
        if let Some(port) = ports.get_mut(port)
            && port.open_origins.get(origin) == Some(&session)
        {
            self.take_error()?;
            port.open_origins.remove(origin);
        }
        Ok(())
    }

    fn read(
        &self,
        origin: &str,
        port: &str,
        session: SerialSessionId,
    ) -> Result<Option<Vec<u8>>, SerialError> {
        let mut ports = self.ports.lock().unwrap();
        let port = ports.get_mut(port).ok_or_else(not_found)?;
        if port.open_origins.get(origin) != Some(&session) {
            return Err(invalid_state("The serial port is not open"));
        }
        self.take_error()?;
        Ok(port.reads.pop_front())
    }

    fn write(
        &self,
        origin: &str,
        port: &str,
        session: SerialSessionId,
        bytes: &[u8],
    ) -> Result<(), SerialError> {
        let mut ports = self.ports.lock().unwrap();
        let port = ports.get_mut(port).ok_or_else(not_found)?;
        if port.open_origins.get(origin) != Some(&session) {
            return Err(invalid_state("The serial port is not open"));
        }
        self.take_error()?;
        port.writes.push(bytes.to_vec());
        Ok(())
    }

    fn get_signals(
        &self,
        origin: &str,
        port: &str,
        session: SerialSessionId,
    ) -> Result<SerialInputSignals, SerialError> {
        let ports = self.ports.lock().unwrap();
        let port = ports.get(port).ok_or_else(not_found)?;
        if port.open_origins.get(origin) != Some(&session) {
            return Err(invalid_state("The serial port is not open"));
        }
        self.take_error()?;
        Ok(port.input_signals.clone())
    }

    fn set_signals(
        &self,
        origin: &str,
        port: &str,
        session: SerialSessionId,
        signals: &SerialOutputSignals,
    ) -> Result<(), SerialError> {
        let mut ports = self.ports.lock().unwrap();
        let port = ports.get_mut(port).ok_or_else(not_found)?;
        if port.open_origins.get(origin) != Some(&session) {
            return Err(invalid_state("The serial port is not open"));
        }
        self.take_error()?;
        port.output_signals = port.output_signals.updated(signals);
        Ok(())
    }

    fn retire(&self, origin: &str, port: &str, session: SerialSessionId) {
        let mut ports = self.ports.lock().unwrap();
        if let Some(port) = ports.get_mut(port)
            && port.open_origins.get(origin) == Some(&session)
        {
            port.open_origins.remove(origin);
        }
    }

    fn forget(&self, origin: &str, port: &str) -> Result<(), SerialError> {
        self.take_error()?;
        let mut ports = self.ports.lock().unwrap();
        let port = ports.get_mut(port).ok_or_else(not_found)?;
        if port.open_origins.contains_key(origin) {
            return Err(invalid_state("An open serial port cannot be forgotten"));
        }
        port.permitted_origins.remove(origin);
        Ok(())
    }
}

fn not_found() -> SerialError {
    SerialError {
        name: "NotFoundError".into(),
        message: "The serial port is unavailable".into(),
    }
}
fn invalid_state(message: &str) -> SerialError {
    SerialError {
        name: "InvalidStateError".into(),
        message: message.into(),
    }
}

struct OpenSession {
    handle: u64,
    provider: Arc<dyn SerialProvider>,
    origin: String,
    token: SerialSessionId,
}

pub(super) struct State {
    pub(super) provider: Arc<dyn SerialProvider>,
    sessions: RefCell<HashMap<(usize, String), Rc<OpenSession>>>,
    next_handle: Cell<u64>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            provider: Arc::new(DeterministicSerialProvider::default()),
            sessions: RefCell::new(HashMap::new()),
            next_handle: Cell::new(0),
        }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        for ((_, port), session) in self.sessions.get_mut().drain() {
            session
                .provider
                .retire(&session.origin, &port, session.token);
        }
    }
}

impl State {
    #[cfg(test)]
    pub(super) fn session_token_for_test(&self, document: usize, port: &str) -> SerialSessionId {
        self.session(document, port)
            .expect("test owns an open session")
            .token
    }

    fn session(&self, document: usize, port: &str) -> Result<Rc<OpenSession>, SerialError> {
        self.sessions
            .borrow()
            .get(&(document, port.to_owned()))
            .cloned()
            .ok_or_else(|| invalid_state("This Document does not own an open serial session"))
    }

    fn session_for_handle(
        &self,
        document: usize,
        port: &str,
        handle: u64,
    ) -> Result<Rc<OpenSession>, SerialError> {
        let session = self.session(document, port)?;
        if session.handle != handle {
            return Err(invalid_state("This serial connection is no longer open"));
        }
        Ok(session)
    }

    pub(super) fn open(
        &self,
        document: usize,
        origin: &str,
        port: &str,
        options: &SerialOptions,
    ) -> Result<u64, SerialError> {
        let provider = Arc::clone(&self.provider);
        if self
            .sessions
            .borrow()
            .get(&(document, port.to_owned()))
            .is_some_and(|old| !Arc::ptr_eq(&old.provider, &provider))
        {
            return Err(invalid_state(
                "Close the previous provider session before opening another",
            ));
        }
        // The provider atomically rejects a still-open session. A successful
        // reconnect replaces a stale record whose token was invalidated.
        // Reserve before opening: exhaustion must not leave an unowned port.
        let handle = self.next_handle.get();
        self.next_handle
            .set(handle.checked_add(1).ok_or_else(|| SerialError {
                name: "OperationError".into(),
                message: "Host serial connection identities exhausted".into(),
            })?);
        let token = provider.open(origin, port, options)?;
        self.sessions.borrow_mut().insert(
            (document, port.to_owned()),
            Rc::new(OpenSession {
                handle,
                provider,
                origin: origin.to_owned(),
                token,
            }),
        );
        Ok(handle)
    }

    fn close(&self, document: usize, port: &str, handle: u64) -> Result<(), SerialError> {
        let Ok(session) = self.session_for_handle(document, port, handle) else {
            // An old JS port can finish its local cleanup after another object
            // has opened the same port. Never close the replacement connection.
            return Ok(());
        };
        session
            .provider
            .close(&session.origin, port, session.token)?;
        let key = (document, port.to_owned());
        let mut sessions = self.sessions.borrow_mut();
        if sessions
            .get(&key)
            .is_some_and(|current| Rc::ptr_eq(current, &session))
        {
            sessions.remove(&key);
        }
        Ok(())
    }

    pub(super) fn retire_document(&self, document: usize) {
        // Remove ownership before external callbacks. No later cleanup retry may
        // target a connection opened after this Document retired.
        let owned: Vec<_> = {
            let mut sessions = self.sessions.borrow_mut();
            let keys: Vec<_> = sessions
                .keys()
                .filter(|(id, _)| *id == document)
                .cloned()
                .collect();
            keys.into_iter()
                .filter_map(|key| sessions.remove(&key).map(|session| (key.1, session)))
                .collect()
        };
        for (port, session) in owned {
            session
                .provider
                .retire(&session.origin, &port, session.token);
        }
    }
}

fn error_json(error: SerialError) -> serde_json::Value {
    let name = match error.name.as_str() {
        "AbortError" | "InvalidStateError" | "NetworkError" | "NotAllowedError"
        | "NotFoundError" | "OperationError" => error.name,
        _ => "OperationError".into(),
    };
    serde_json::json!({"error":name,"message":error.message})
}

fn validate_info(info: SerialPortInfo) -> Result<SerialPortInfo, SerialError> {
    if info.id.is_empty() || (info.usb_product_id.is_some() && info.usb_vendor_id.is_none()) {
        Err(SerialError {
            name: "OperationError".into(),
            message: "The serial provider returned invalid port information".into(),
        })
    } else {
        Ok(info)
    }
}

fn parse_output_signals(json: &str) -> JsResult<SerialOutputSignals> {
    let signals: SerialOutputSignals = serde_json::from_str(json)
        .map_err(|_| JsNativeError::typ().with_message("Invalid serial signals"))?;
    if signals.is_empty() {
        return Err(JsNativeError::typ()
            .with_message("At least one serial output signal is required")
            .into());
    }
    Ok(signals)
}

fn operation_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let operation = string_argument(args.first(), "", context)?;
    let document_id = parse_node_id(args.get(1), context)?;
    ensure_same_origin_document(context, document_id)?;
    let port_id = string_argument(args.get(2), "", context)?;
    let json = string_argument(args.get(3), "{}", context)?;
    let handle = string_argument(args.get(4), "", context)?;
    let result = with_host_state(|host| {
        let host = host.borrow();
        let Some(url) = host.base_url_for_document(document_id) else {
            return Ok(error_json(SerialError {
                name: "NotAllowedError".into(),
                message: "Web Serial requires a tuple origin".into(),
            }));
        };
        let origin = format!("{}://{}", url.scheme(), url.authority());
        let handle = if matches!(
            operation.as_str(),
            "close" | "read" | "write" | "getSignals" | "setSignals"
        ) {
            match handle.parse::<u64>() {
                Ok(handle) => Some(handle),
                Err(_) => return Ok(error_json(invalid_state("No serial connection identity"))),
            }
        } else {
            None
        };
        let session = if matches!(
            operation.as_str(),
            "read" | "write" | "getSignals" | "setSignals"
        ) {
            match host.web_serial.session_for_handle(
                document_id,
                &port_id,
                handle.expect("connection identity checked"),
            ) {
                Ok(session) => Some(session),
                Err(error) => return Ok(error_json(error)),
            }
        } else {
            None
        };
        let provider = session
            .as_ref()
            .map_or(&host.web_serial.provider, |session| &session.provider);
        let origin = session
            .as_ref()
            .map_or(origin, |session| session.origin.clone());
        let result = match operation.as_str() {
            "get" => {
                let ports = provider
                    .get_ports(&origin)
                    .into_iter()
                    .map(validate_info)
                    .collect::<Result<Vec<_>, _>>();
                ports.map(|ports| serde_json::json!({"ports":ports}))
            }
            "request" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase", deny_unknown_fields)]
                struct Request {
                    #[serde(default)]
                    filters: Vec<SerialPortFilter>,
                    #[serde(default)]
                    allowed_bluetooth_service_class_ids: Vec<String>,
                }
                let request: Request = serde_json::from_str(&json)
                    .map_err(|_| JsNativeError::typ().with_message("Invalid serial request"))?;
                provider
                    .request_port(
                        &origin,
                        &request.filters,
                        &request.allowed_bluetooth_service_class_ids,
                    )
                    .and_then(validate_info)
                    .map(|port| serde_json::json!({"port":port}))
            }
            "open" => {
                let options: SerialOptions = serde_json::from_str(&json)
                    .map_err(|_| JsNativeError::typ().with_message("Invalid serial options"))?;
                host.web_serial
                    .open(document_id, &origin, &port_id, &options)
                    .map(|handle| serde_json::json!({"ok":true,"session":handle.to_string()}))
            }
            "close" => host
                .web_serial
                .close(
                    document_id,
                    &port_id,
                    handle.expect("connection identity checked"),
                )
                .map(|()| serde_json::json!({"ok":true})),
            "read" => provider
                .read(
                    &origin,
                    &port_id,
                    session.as_ref().expect("owned session checked").token,
                )
                .map(|bytes| serde_json::json!({"bytes":bytes})),
            "write" => {
                let bytes: Vec<u8> = serde_json::from_str(&json)
                    .map_err(|_| JsNativeError::typ().with_message("Invalid serial bytes"))?;
                provider
                    .write(
                        &origin,
                        &port_id,
                        session.as_ref().expect("owned session checked").token,
                        &bytes,
                    )
                    .map(|()| serde_json::json!({"ok":true}))
            }
            "getSignals" => provider
                .get_signals(
                    &origin,
                    &port_id,
                    session.as_ref().expect("owned session checked").token,
                )
                .map(|signals| serde_json::json!({"signals":signals})),
            "setSignals" => {
                let signals = parse_output_signals(&json)?;
                provider
                    .set_signals(
                        &origin,
                        &port_id,
                        session.as_ref().expect("owned session checked").token,
                        &signals,
                    )
                    .map(|()| serde_json::json!({"ok":true}))
            }
            "forget" => provider
                .forget(&origin, &port_id)
                .map(|()| serde_json::json!({"ok":true})),
            _ => {
                return Err(JsNativeError::typ()
                    .with_message("Unknown Web Serial operation")
                    .into());
            }
        };
        Ok(match result {
            Ok(value) => value,
            Err(error) => error_json(error),
        })
    })?;
    Ok(JsValue::from(JsString::from(result.to_string().as_str())))
}

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_serial"),
        5,
        NativeFunction::from_copy_closure(operation_native),
    )
}

impl JsRuntime {
    /// Replaces the serial backend inherited by subsequently created workers.
    pub fn set_serial_provider(&mut self, provider: Arc<dyn SerialProvider>) {
        self.host_state.borrow_mut().web_serial.provider = provider;
    }
}

#[cfg(test)]
mod session_tests {
    use super::*;

    fn provider() -> Arc<DeterministicSerialProvider> {
        let provider = Arc::new(DeterministicSerialProvider::default());
        provider.add_port(SerialPortInfo {
            id: "host-id".into(),
            usb_vendor_id: None,
            usb_product_id: None,
            bluetooth_service_class_id: None,
            connected: true,
        });
        provider
            .request_port("https://host.test", &[], &[])
            .unwrap();
        provider
    }

    fn options() -> SerialOptions {
        SerialOptions {
            baud_rate: 9600,
            data_bits: 8,
            stop_bits: 1,
            parity: "none".into(),
            buffer_size: 255,
            flow_control: "none".into(),
        }
    }

    #[test]
    fn host_handles_remain_distinct_when_provider_ids_repeat() {
        let first = provider();
        let second = provider();
        let mut state = State::default();
        state.provider = first;
        let old = state
            .open(42, "https://host.test", "host-id", &options())
            .unwrap();
        let old_token = state.session(42, "host-id").unwrap().token;
        state.close(42, "host-id", old).unwrap();
        state.provider = second.clone();
        let current = state
            .open(42, "https://host.test", "host-id", &options())
            .unwrap();
        let current_token = state.session(42, "host-id").unwrap().token;
        assert_eq!(old_token, current_token);
        assert_ne!(old, current);
        assert!(state.session_for_handle(42, "host-id", old).is_err());
        state.close(42, "host-id", old).unwrap();
        second
            .write("https://host.test", "host-id", current_token, &[7])
            .unwrap();
        assert_eq!(second.writes("host-id"), vec![vec![7]]);
    }

    #[test]
    fn exhausted_host_handle_does_not_open_an_unowned_backend_session() {
        let provider = provider();
        let mut state = State::default();
        state.provider = provider.clone();
        state.next_handle.set(u64::MAX);
        assert_eq!(
            state
                .open(42, "https://host.test", "host-id", &options())
                .unwrap_err()
                .name,
            "OperationError"
        );
        let token = provider
            .open("https://host.test", "host-id", &options())
            .unwrap();
        provider
            .close("https://host.test", "host-id", token)
            .unwrap();
    }
}
