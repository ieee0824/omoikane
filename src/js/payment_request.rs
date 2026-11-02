//! Host-owned provider boundary for the Payment Request API.

use super::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// A validated payment method supplied by the merchant.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PaymentMethodData {
    /// Standardized payment method identifier or absolute URL.
    pub supported_methods: String,
    /// JSON-serialized method-specific data.
    #[serde(default)]
    pub data: serde_json::Value,
}

/// An owned request passed across the embedder boundary.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PaymentRequestData {
    /// Stable request identifier.
    pub id: String,
    /// Serialized caller origin.
    #[serde(default)]
    pub origin: String,
    /// Accepted payment methods.
    pub methods: Vec<PaymentMethodData>,
    /// Validated PaymentDetailsInit represented as JSON.
    pub details: serde_json::Value,
    /// Validated PaymentOptions represented as JSON.
    pub options: serde_json::Value,
}

/// A payment selected by the host UI.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PaymentSelection {
    /// Selected method identifier.
    pub method_name: String,
    /// Method-specific response details.
    #[serde(default)]
    pub details: serde_json::Value,
    /// Optional payer name.
    pub payer_name: Option<String>,
    /// Optional payer email.
    pub payer_email: Option<String>,
    /// Optional payer phone.
    pub payer_phone: Option<String>,
}

/// Failure reported by a payment provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaymentError {
    /// DOMException name exposed to script.
    pub name: String,
    /// Human-readable failure detail.
    pub message: String,
}

/// Embedder contract for payment capability and UI operations.
pub trait PaymentProvider: Send + Sync {
    /// Reports whether any requested method can be handled.
    fn can_make_payment(&self, request: &PaymentRequestData) -> bool;
    /// Presents the payment UI and returns an explicit host result.
    fn show(&self, request: PaymentRequestData) -> Result<PaymentSelection, PaymentError>;
    /// Cancels UI owned by a request or retired document.
    fn abort(&self, _document_id: usize, _request_id: &str) {}
    /// Notifies the provider that the merchant completed the response.
    fn complete(&self, _request_id: &str, _result: &str) -> Result<(), PaymentError> {
        Ok(())
    }
}

/// Deterministic provider for headless embedders and tests.
#[derive(Debug)]
pub struct DeterministicPaymentProvider {
    supported: Mutex<bool>,
    requests: Mutex<Vec<PaymentRequestData>>,
    next_result: Mutex<Option<Result<PaymentSelection, PaymentError>>>,
    aborted: Mutex<Vec<String>>,
    completed: Mutex<Vec<(String, String)>>,
}

impl Default for DeterministicPaymentProvider {
    fn default() -> Self {
        Self {
            supported: Mutex::new(true),
            requests: Mutex::new(Vec::new()),
            next_result: Mutex::new(None),
            aborted: Mutex::new(Vec::new()),
            completed: Mutex::new(Vec::new()),
        }
    }
}

impl DeterministicPaymentProvider {
    /// Enables or disables capability discovery.
    pub fn set_supported(&self, supported: bool) {
        *self.supported.lock().unwrap() = supported;
    }
    /// Supplies the result consumed by the next `show()` call.
    pub fn respond_to_next(&self, result: Result<PaymentSelection, PaymentError>) {
        *self.next_result.lock().unwrap() = Some(result);
    }
    /// Returns requests observed by the provider.
    pub fn requests(&self) -> Vec<PaymentRequestData> {
        self.requests.lock().unwrap().clone()
    }
    /// Returns request identifiers aborted by script or lifecycle retirement.
    pub fn aborted_requests(&self) -> Vec<String> {
        self.aborted.lock().unwrap().clone()
    }
    /// Returns completion notifications in call order.
    pub fn completions(&self) -> Vec<(String, String)> {
        self.completed.lock().unwrap().clone()
    }
}

impl PaymentProvider for DeterministicPaymentProvider {
    fn can_make_payment(&self, request: &PaymentRequestData) -> bool {
        *self.supported.lock().unwrap()
            && request.methods.iter().any(|method| {
                matches!(
                    method.supported_methods.as_str(),
                    "basic-card" | "https://apple.com/apple-pay"
                )
            })
    }
    fn show(&self, request: PaymentRequestData) -> Result<PaymentSelection, PaymentError> {
        self.requests.lock().unwrap().push(request);
        self.next_result.lock().unwrap().take().unwrap_or_else(|| {
            Err(PaymentError {
                name: "Pending".into(),
                message: "The payment provider has not responded".into(),
            })
        })
    }
    fn abort(&self, _document_id: usize, request_id: &str) {
        self.aborted.lock().unwrap().push(request_id.to_owned());
    }
    fn complete(&self, request_id: &str, result: &str) -> Result<(), PaymentError> {
        self.completed
            .lock()
            .unwrap()
            .push((request_id.to_owned(), result.to_owned()));
        Ok(())
    }
}

struct PendingPayment {
    request_id: String,
    result: Result<PaymentSelection, PaymentError>,
}

pub(super) struct State {
    provider: Arc<dyn PaymentProvider>,
    pending: HashMap<usize, PendingPayment>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            provider: Arc::new(DeterministicPaymentProvider::default()),
            pending: HashMap::new(),
        }
    }
}

impl State {
    pub(super) fn retire_document(&mut self, document_id: usize) {
        if let Some(pending) = self.pending.remove(&document_id) {
            self.provider.abort(document_id, &pending.request_id);
        }
    }
}

fn error(error: PaymentError) -> serde_json::Value {
    let name = match error.name.as_str() {
        "AbortError" | "NotAllowedError" | "NotSupportedError" | "OperationError" => error.name,
        _ => "OperationError".to_owned(),
    };
    serde_json::json!({"error": name, "message": error.message})
}

fn operation_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let operation = string_argument(args.first(), "", context)?;
    let document_id = parse_node_id(args.get(1), context)?;
    ensure_same_origin_document(context, document_id)?;
    let json = string_argument(args.get(2), "{}", context)?;
    let result = with_host_state(|host| {
        let mut host = host.borrow_mut();
        if !host.document_is_active(document_id) {
            return Ok(
                serde_json::json!({"error":"InvalidStateError","message":"The document is not fully active"}),
            );
        }
        let Some(url) = host.base_url_for_document(document_id) else {
            return Ok(
                serde_json::json!({"error":"SecurityError","message":"Payment Request requires a tuple origin"}),
            );
        };
        match operation.as_str() {
            "canMakePayment" | "show" => {
                let mut request: PaymentRequestData = serde_json::from_str(&json)
                    .map_err(|_| JsNativeError::typ().with_message("Invalid payment request"))?;
                request.origin = format!("{}://{}", url.scheme(), url.authority());
                if operation == "canMakePayment" {
                    return Ok(
                        serde_json::json!({"supported": host.payment_request.provider.can_make_payment(&request)}),
                    );
                }
                if host.payment_request.pending.contains_key(&document_id) {
                    return Ok(
                        serde_json::json!({"error":"AbortError","message":"Another payment request is showing"}),
                    );
                }
                if !host.payment_request.provider.can_make_payment(&request) {
                    return Ok(
                        serde_json::json!({"error":"NotSupportedError","message":"No supported payment method"}),
                    );
                }
                let request_id = request.id.clone();
                let result = host.payment_request.provider.show(request);
                host.payment_request
                    .pending
                    .insert(document_id, PendingPayment { request_id, result });
                Ok(serde_json::json!({"ok":true}))
            }
            "result" => {
                let Some(pending) = host.payment_request.pending.remove(&document_id) else {
                    return Ok(
                        serde_json::json!({"error":"AbortError","message":"The payment request was cancelled"}),
                    );
                };
                if matches!(&pending.result, Err(failure) if failure.name == "Pending") {
                    host.payment_request.pending.insert(document_id, pending);
                    return Ok(serde_json::json!({"pending":true}));
                }
                match pending.result {
                    Ok(selection) => Ok(serde_json::to_value(selection).unwrap()),
                    Err(failure) => Ok(error(failure)),
                }
            }
            "abort" => {
                let Some(pending) = host.payment_request.pending.remove(&document_id) else {
                    return Ok(
                        serde_json::json!({"error":"InvalidStateError","message":"The request is not interactive"}),
                    );
                };
                host.payment_request
                    .provider
                    .abort(document_id, &pending.request_id);
                Ok(serde_json::json!({"ok":true}))
            }
            "complete" => {
                let value: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
                let id = value.get("id").and_then(|v| v.as_str()).unwrap_or_default();
                let status = value
                    .get("result")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                match host.payment_request.provider.complete(id, status) {
                    Ok(()) => Ok(serde_json::json!({"ok":true})),
                    Err(failure) => Ok(error(failure)),
                }
            }
            _ => Err(JsNativeError::typ()
                .with_message("Unknown Payment Request operation")
                .into()),
        }
    })?;
    Ok(JsValue::from(JsString::from(result.to_string().as_str())))
}

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_payment_request"),
        3,
        NativeFunction::from_copy_closure(operation_native),
    )
}

impl JsRuntime {
    /// Replaces the provider used by subsequent payment requests.
    pub fn set_payment_provider(&mut self, provider: Arc<dyn PaymentProvider>) {
        let mut host = self.host_state.borrow_mut();
        let documents = host
            .payment_request
            .pending
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for document in documents {
            host.payment_request.retire_document(document);
        }
        host.payment_request.provider = provider;
    }
}
