//! Host-owned authenticator boundary for Web Authentication.

use super::*;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// A normalized credential creation request passed to the embedder.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebAuthnCreationRequest {
    /// Serialized caller origin.
    pub origin: String,
    /// Effective relying-party identifier.
    pub rp_id: String,
    /// Human-readable relying-party name.
    pub rp_name: String,
    /// Challenge bytes supplied by the relying party.
    pub challenge: Vec<u8>,
    /// Opaque user identifier.
    pub user_id: Vec<u8>,
    /// Account name.
    pub user_name: String,
    /// Account display name.
    pub user_display_name: String,
    /// Accepted COSE algorithms, in preference order.
    pub algorithms: Vec<i32>,
    /// Credential identifiers excluded from creation.
    pub exclude_credentials: Vec<Vec<u8>>,
    /// Requested user-verification policy.
    pub user_verification: String,
    /// Requested resident-key policy.
    pub resident_key: String,
    /// Requested authenticator attachment, when constrained.
    pub authenticator_attachment: Option<String>,
    /// Attestation conveyance preference.
    pub attestation: String,
    /// Caller-provided authenticator hints.
    pub hints: Vec<String>,
    /// Effective request timeout in milliseconds.
    pub timeout: Option<u32>,
    /// Whether the caller had transient user activation at dispatch.
    pub transient_user_activation: bool,
}

/// A normalized credential assertion request passed to the embedder.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebAuthnAssertionRequest {
    /// Serialized caller origin.
    pub origin: String,
    /// Effective relying-party identifier.
    pub rp_id: String,
    /// Challenge bytes supplied by the relying party.
    pub challenge: Vec<u8>,
    /// Credential identifiers accepted by the relying party.
    pub allow_credentials: Vec<Vec<u8>>,
    /// Requested user-verification policy.
    pub user_verification: String,
    /// Credential mediation requirement.
    pub mediation: String,
    /// Caller-provided authenticator hints.
    pub hints: Vec<String>,
    /// Effective request timeout in milliseconds.
    pub timeout: Option<u32>,
}

/// Authenticator result for credential creation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebAuthnCreationResponse {
    /// Credential identifier.
    pub credential_id: Vec<u8>,
    /// Serialized client data JSON.
    pub client_data_json: Vec<u8>,
    /// Encoded attestation object.
    pub attestation_object: Vec<u8>,
    /// Authenticator data extracted from the attestation.
    pub authenticator_data: Vec<u8>,
    /// DER-encoded public key, when available.
    pub public_key: Option<Vec<u8>>,
    /// COSE algorithm used by the credential.
    pub public_key_algorithm: i32,
    /// Authenticator transports.
    pub transports: Vec<String>,
    /// Authenticator attachment modality.
    pub authenticator_attachment: Option<String>,
    /// Client extension results.
    pub client_extension_results: serde_json::Value,
}

/// Authenticator result for credential assertion.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebAuthnAssertionResponse {
    /// Credential identifier.
    pub credential_id: Vec<u8>,
    /// Serialized client data JSON.
    pub client_data_json: Vec<u8>,
    /// Authenticator data covered by the signature.
    pub authenticator_data: Vec<u8>,
    /// Authenticator signature.
    pub signature: Vec<u8>,
    /// User handle, when returned by the authenticator.
    pub user_handle: Option<Vec<u8>>,
    /// Authenticator attachment modality.
    pub authenticator_attachment: Option<String>,
    /// Client extension results.
    pub client_extension_results: serde_json::Value,
}

/// Failure reported by the authenticator boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebAuthnError {
    /// DOMException name exposed to the caller.
    pub name: String,
    /// Human-readable failure detail.
    pub message: String,
}

/// Embedder contract for WebAuthn capability and authenticator operations.
pub trait WebAuthnProvider: Send + Sync {
    /// Reports supported WebAuthn client capabilities.
    fn capabilities(&self) -> Vec<String> {
        Vec::new()
    }
    /// Creates a public-key credential.
    fn create(
        &self,
        request: WebAuthnCreationRequest,
    ) -> Result<WebAuthnCreationResponse, WebAuthnError>;
    /// Gets an assertion for an existing public-key credential.
    fn get(
        &self,
        request: WebAuthnAssertionRequest,
    ) -> Result<WebAuthnAssertionResponse, WebAuthnError>;
    /// Cancels an in-flight request owned by a retired document.
    fn cancel(&self, _document_id: usize) {}
}

/// Deterministic authenticator provider for embedders and tests.
#[derive(Debug, Default)]
pub struct DeterministicWebAuthnProvider {
    creation_requests: Mutex<Vec<WebAuthnCreationRequest>>,
    assertion_requests: Mutex<Vec<WebAuthnAssertionRequest>>,
    next_creation: Mutex<Option<Result<WebAuthnCreationResponse, WebAuthnError>>>,
    next_assertion: Mutex<Option<Result<WebAuthnAssertionResponse, WebAuthnError>>>,
    capabilities: Mutex<Vec<String>>,
    cancelled_documents: Mutex<Vec<usize>>,
}

impl DeterministicWebAuthnProvider {
    /// Sets the response consumed by the next creation request.
    pub fn respond_to_next_creation(
        &self,
        response: Result<WebAuthnCreationResponse, WebAuthnError>,
    ) {
        *self.next_creation.lock().unwrap() = Some(response);
    }

    /// Sets the response consumed by the next assertion request.
    pub fn respond_to_next_assertion(
        &self,
        response: Result<WebAuthnAssertionResponse, WebAuthnError>,
    ) {
        *self.next_assertion.lock().unwrap() = Some(response);
    }

    /// Replaces the reported client capability names.
    pub fn set_capabilities(&self, capabilities: impl IntoIterator<Item = String>) {
        *self.capabilities.lock().unwrap() = capabilities.into_iter().collect();
    }

    /// Returns owned creation requests observed so far.
    pub fn creation_requests(&self) -> Vec<WebAuthnCreationRequest> {
        self.creation_requests.lock().unwrap().clone()
    }

    /// Returns owned assertion requests observed so far.
    pub fn assertion_requests(&self) -> Vec<WebAuthnAssertionRequest> {
        self.assertion_requests.lock().unwrap().clone()
    }

    /// Returns document identifiers cancelled by lifecycle retirement.
    pub fn cancelled_documents(&self) -> Vec<usize> {
        self.cancelled_documents.lock().unwrap().clone()
    }
}

fn unavailable() -> WebAuthnError {
    WebAuthnError {
        name: "NotAllowedError".into(),
        message: "No authenticator accepted the request".into(),
    }
}

impl WebAuthnProvider for DeterministicWebAuthnProvider {
    fn capabilities(&self) -> Vec<String> {
        self.capabilities.lock().unwrap().clone()
    }

    fn create(
        &self,
        request: WebAuthnCreationRequest,
    ) -> Result<WebAuthnCreationResponse, WebAuthnError> {
        self.creation_requests.lock().unwrap().push(request);
        self.next_creation
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| Err(unavailable()))
    }

    fn get(
        &self,
        request: WebAuthnAssertionRequest,
    ) -> Result<WebAuthnAssertionResponse, WebAuthnError> {
        self.assertion_requests.lock().unwrap().push(request);
        self.next_assertion
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| Err(unavailable()))
    }

    fn cancel(&self, document_id: usize) {
        self.cancelled_documents.lock().unwrap().push(document_id);
    }
}

pub(super) struct State {
    provider: Arc<dyn WebAuthnProvider>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            provider: Arc::new(DeterministicWebAuthnProvider::default()),
        }
    }
}

impl State {
    pub(super) fn retire_document(&self, document_id: usize) {
        self.provider.cancel(document_id);
    }
}

fn error_json(error: WebAuthnError) -> serde_json::Value {
    let name = match error.name.as_str() {
        "AbortError" | "ConstraintError" | "InvalidStateError" | "NotAllowedError"
        | "NotSupportedError" | "OperationError" | "SecurityError" => error.name,
        _ => "OperationError".to_owned(),
    };
    serde_json::json!({"error": name, "message": error.message})
}

fn is_known_capability(capability: &str) -> bool {
    matches!(
        capability,
        "conditionalCreate"
            | "conditionalGet"
            | "hybridTransport"
            | "immediateGet"
            | "passkeyPlatformAuthenticator"
            | "relatedOrigins"
            | "signalAllAcceptedCredentials"
            | "signalCurrentUserDetails"
            | "signalUnknownCredential"
            | "userVerifyingPlatformAuthenticator"
    ) || capability.starts_with("extension:")
}

fn validate_rp_id(origin: &crate::http::Url, rp_id: &str) -> Result<(), WebAuthnError> {
    let origin_host = origin.host();
    let valid = rp_id == origin_host
        || (origin_host
            .strip_suffix(rp_id)
            .is_some_and(|prefix| prefix.ends_with('.'))
            && !crate::http::is_public_suffix(rp_id));
    if valid {
        Ok(())
    } else {
        Err(WebAuthnError {
            name: "SecurityError".into(),
            message: "The relying-party ID is not valid for this origin".into(),
        })
    }
}

fn operation_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let operation = string_argument(args.first(), "", context)?;
    let document_id = parse_node_id(args.get(1), context)?;
    ensure_same_origin_document(context, document_id)?;
    let result = with_host_state(|host| {
        let host = host.borrow();
        if !host.document_is_active(document_id) {
            return Ok(
                serde_json::json!({"error":"InvalidStateError", "message":"The document is not fully active"}),
            );
        }
        if operation == "capabilities" {
            let mut capabilities = host.web_authn.provider.capabilities();
            capabilities.retain(|capability| is_known_capability(capability));
            capabilities.sort();
            capabilities.dedup();
            return Ok(serde_json::json!({"capabilities": capabilities}));
        }
        let Some(url) = host.base_url_for_document(document_id) else {
            return Ok(
                serde_json::json!({"error":"SecurityError", "message":"The document has no tuple origin"}),
            );
        };
        let origin = format!("{}://{}", url.scheme(), url.authority());
        let json = string_argument(args.get(2), "", context)?;
        match operation.as_str() {
            "create" => {
                if !host.feature_enabled_in_document(
                    document_id,
                    permissions_policy::PolicyFeature::PublicKeyCredentialsCreate,
                ) {
                    return Ok(
                        serde_json::json!({"error":"NotAllowedError", "message":"Credential creation is disabled by Permissions Policy"}),
                    );
                }
                let mut request: WebAuthnCreationRequest =
                    serde_json::from_str(&json).map_err(|_| {
                        JsNativeError::typ().with_message("Invalid WebAuthn creation request")
                    })?;
                request.origin = origin;
                if let Err(error) = validate_rp_id(&url, &request.rp_id) {
                    return Ok(error_json(error));
                }
                if document_id != host.document.identity()
                    && host
                        .document_security_origins
                        .get(&document_id)
                        .zip(
                            host.document_security_origins
                                .get(&host.document.identity()),
                        )
                        .is_some_and(|(child, top)| child != top)
                    && !request.transient_user_activation
                {
                    return Ok(
                        serde_json::json!({"error":"NotAllowedError", "message":"Cross-origin credential creation requires transient user activation"}),
                    );
                }
                match host.web_authn.provider.create(request) {
                    Ok(response) => Ok(serde_json::json!({"creation": response})),
                    Err(error) => Ok(error_json(error)),
                }
            }
            "get" => {
                if !host.feature_enabled_in_document(
                    document_id,
                    permissions_policy::PolicyFeature::PublicKeyCredentialsGet,
                ) {
                    return Ok(
                        serde_json::json!({"error":"NotAllowedError", "message":"Credential retrieval is disabled by Permissions Policy"}),
                    );
                }
                let mut request: WebAuthnAssertionRequest =
                    serde_json::from_str(&json).map_err(|_| {
                        JsNativeError::typ().with_message("Invalid WebAuthn assertion request")
                    })?;
                request.origin = origin;
                if let Err(error) = validate_rp_id(&url, &request.rp_id) {
                    return Ok(error_json(error));
                }
                match host.web_authn.provider.get(request) {
                    Ok(response) => Ok(serde_json::json!({"assertion": response})),
                    Err(error) => Ok(error_json(error)),
                }
            }
            _ => Err(JsNativeError::typ()
                .with_message("Unknown WebAuthn operation")
                .into()),
        }
    })?;
    Ok(JsValue::from(JsString::from(result.to_string().as_str())))
}

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_web_authn"),
        3,
        NativeFunction::from_copy_closure(operation_native),
    )
}

impl JsRuntime {
    /// Replaces the authenticator provider used by subsequent WebAuthn requests.
    pub fn set_web_authn_provider(&mut self, provider: Arc<dyn WebAuthnProvider>) {
        self.host_state.borrow_mut().web_authn.provider = provider;
    }
}
