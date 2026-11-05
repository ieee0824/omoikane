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

/// Host control-line output values.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SerialOutputSignals {
    /// Data Terminal Ready output.
    pub data_terminal_ready: bool,
    /// Request To Send output.
    pub request_to_send: bool,
    /// Break output.
    #[serde(rename = "break")]
    pub break_signal: bool,
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

/// Embedder contract for serial enumeration, permission, and byte I/O.
pub trait SerialProvider: Send + Sync {
    fn get_ports(&self, origin: &str) -> Vec<SerialPortInfo>;
    fn request_port(
        &self,
        origin: &str,
        filters: &[SerialPortFilter],
        allowed_bluetooth_services: &[String],
    ) -> Result<SerialPortInfo, SerialError>;
    fn open(&self, origin: &str, port: &str, options: &SerialOptions) -> Result<(), SerialError>;
    fn close(&self, origin: &str, port: &str) -> Result<(), SerialError>;
    fn read(&self, origin: &str, port: &str) -> Result<Option<Vec<u8>>, SerialError>;
    fn write(&self, origin: &str, port: &str, bytes: &[u8]) -> Result<(), SerialError>;
    fn get_signals(&self, origin: &str, port: &str) -> Result<SerialInputSignals, SerialError>;
    fn set_signals(
        &self,
        origin: &str,
        port: &str,
        signals: &SerialOutputSignals,
    ) -> Result<(), SerialError>;
    fn forget(&self, origin: &str, port: &str) -> Result<(), SerialError>;
    fn retire(&self, _origin: &str) {}
}

#[derive(Debug)]
struct DeterministicPort {
    info: SerialPortInfo,
    permitted_origins: HashSet<String>,
    open_origins: HashSet<String>,
    reads: VecDeque<Vec<u8>>,
    writes: Vec<Vec<u8>>,
    input_signals: SerialInputSignals,
    output_signals: SerialOutputSignals,
}

/// Deterministic in-memory serial backend for headless embedders and tests.
#[derive(Debug, Default)]
pub struct DeterministicSerialProvider {
    ports: Mutex<HashMap<String, DeterministicPort>>,
    next_error: Mutex<Option<SerialError>>,
}

impl DeterministicSerialProvider {
    /// Adds or replaces an emulated port.
    pub fn add_port(&self, info: SerialPortInfo) {
        self.ports.lock().unwrap().insert(
            info.id.clone(),
            DeterministicPort {
                info,
                permitted_origins: HashSet::new(),
                open_origins: HashSet::new(),
                reads: VecDeque::new(),
                writes: Vec::new(),
                input_signals: SerialInputSignals::default(),
                output_signals: SerialOutputSignals::default(),
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

    fn open(&self, origin: &str, port: &str, _options: &SerialOptions) -> Result<(), SerialError> {
        self.take_error()?;
        let mut ports = self.ports.lock().unwrap();
        let port = ports.get_mut(port).ok_or_else(not_found)?;
        if !port.info.connected || !port.permitted_origins.contains(origin) {
            return Err(not_found());
        }
        if !port.open_origins.insert(origin.to_owned()) {
            return Err(SerialError {
                name: "InvalidStateError".into(),
                message: "The serial port is already open".into(),
            });
        }
        Ok(())
    }

    fn close(&self, origin: &str, port: &str) -> Result<(), SerialError> {
        self.take_error()?;
        let mut ports = self.ports.lock().unwrap();
        let port = ports.get_mut(port).ok_or_else(not_found)?;
        port.open_origins.remove(origin);
        Ok(())
    }

    fn read(&self, origin: &str, port: &str) -> Result<Option<Vec<u8>>, SerialError> {
        self.take_error()?;
        let mut ports = self.ports.lock().unwrap();
        let port = ports.get_mut(port).ok_or_else(not_found)?;
        if !port.open_origins.contains(origin) {
            return Err(invalid_state("The serial port is not open"));
        }
        Ok(port.reads.pop_front())
    }

    fn write(&self, origin: &str, port: &str, bytes: &[u8]) -> Result<(), SerialError> {
        self.take_error()?;
        let mut ports = self.ports.lock().unwrap();
        let port = ports.get_mut(port).ok_or_else(not_found)?;
        if !port.open_origins.contains(origin) {
            return Err(invalid_state("The serial port is not open"));
        }
        port.writes.push(bytes.to_vec());
        Ok(())
    }

    fn get_signals(&self, origin: &str, port: &str) -> Result<SerialInputSignals, SerialError> {
        self.take_error()?;
        let ports = self.ports.lock().unwrap();
        let port = ports.get(port).ok_or_else(not_found)?;
        if !port.open_origins.contains(origin) {
            return Err(invalid_state("The serial port is not open"));
        }
        Ok(port.input_signals.clone())
    }

    fn set_signals(
        &self,
        origin: &str,
        port: &str,
        signals: &SerialOutputSignals,
    ) -> Result<(), SerialError> {
        self.take_error()?;
        let mut ports = self.ports.lock().unwrap();
        let port = ports.get_mut(port).ok_or_else(not_found)?;
        if !port.open_origins.contains(origin) {
            return Err(invalid_state("The serial port is not open"));
        }
        port.output_signals = signals.clone();
        Ok(())
    }

    fn forget(&self, origin: &str, port: &str) -> Result<(), SerialError> {
        self.take_error()?;
        let mut ports = self.ports.lock().unwrap();
        let port = ports.get_mut(port).ok_or_else(not_found)?;
        if port.open_origins.contains(origin) {
            return Err(invalid_state("An open serial port cannot be forgotten"));
        }
        port.permitted_origins.remove(origin);
        Ok(())
    }

    fn retire(&self, origin: &str) {
        for port in self.ports.lock().unwrap().values_mut() {
            port.open_origins.remove(origin);
        }
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

pub(super) struct State {
    pub(super) provider: Arc<dyn SerialProvider>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            provider: Arc::new(DeterministicSerialProvider::default()),
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

fn operation_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let operation = string_argument(args.first(), "", context)?;
    let document_id = parse_node_id(args.get(1), context)?;
    ensure_same_origin_document(context, document_id)?;
    let port_id = string_argument(args.get(2), "", context)?;
    let json = string_argument(args.get(3), "{}", context)?;
    let result = with_host_state(|host| {
        let host = host.borrow();
        let Some(url) = host.base_url_for_document(document_id) else {
            return Ok(error_json(SerialError {
                name: "NotAllowedError".into(),
                message: "Web Serial requires a tuple origin".into(),
            }));
        };
        let origin = format!("{}://{}", url.scheme(), url.authority());
        let provider = &host.web_serial.provider;
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
                provider
                    .open(&origin, &port_id, &options)
                    .map(|()| serde_json::json!({"ok":true}))
            }
            "close" => provider
                .close(&origin, &port_id)
                .map(|()| serde_json::json!({"ok":true})),
            "read" => provider
                .read(&origin, &port_id)
                .map(|bytes| serde_json::json!({"bytes":bytes})),
            "write" => {
                let bytes: Vec<u8> = serde_json::from_str(&json)
                    .map_err(|_| JsNativeError::typ().with_message("Invalid serial bytes"))?;
                provider
                    .write(&origin, &port_id, &bytes)
                    .map(|()| serde_json::json!({"ok":true}))
            }
            "getSignals" => provider
                .get_signals(&origin, &port_id)
                .map(|signals| serde_json::json!({"signals":signals})),
            "setSignals" => {
                let signals: SerialOutputSignals = serde_json::from_str(&json)
                    .map_err(|_| JsNativeError::typ().with_message("Invalid serial signals"))?;
                provider
                    .set_signals(&origin, &port_id, &signals)
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
        4,
        NativeFunction::from_copy_closure(operation_native),
    )
}

impl JsRuntime {
    /// Replaces the serial backend inherited by subsequently created workers.
    pub fn set_serial_provider(&mut self, provider: Arc<dyn SerialProvider>) {
        self.host_state.borrow_mut().web_serial.provider = provider;
    }
}
