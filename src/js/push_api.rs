//! Host-owned subscription boundary and Service Worker delivery for Push API.

use super::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Stable owner of a push subscription.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PushOwner {
    /// Serialized tuple origin.
    pub origin: String,
    /// Absolute Service Worker scope, or the origin root for `window.pushManager`.
    pub scope: String,
    /// Current Service Worker registration generation when one owns the manager.
    pub registration_id: Option<u64>,
}

/// Normalized subscription options crossing the host boundary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PushSubscriptionOptionsData {
    /// Whether every message must result in a user-visible notification.
    pub user_visible_only: bool,
    /// Owned P-256 application-server public key.
    pub application_server_key: Option<Vec<u8>>,
}

/// Owned subscription returned by the host push service.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PushSubscriptionData {
    /// Unique HTTPS endpoint.
    pub endpoint: String,
    /// Optional expiration timestamp in milliseconds.
    pub expiration_time: Option<f64>,
    /// Subscription creation options.
    pub options: PushSubscriptionOptionsData,
    /// ECDH public key bytes.
    pub p256dh: Vec<u8>,
    /// Authentication secret bytes.
    pub auth: Vec<u8>,
}

/// Failure reported by a push service.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PushError {
    /// DOMException name exposed to script.
    pub name: String,
    /// Human-readable failure detail.
    pub message: String,
}

/// Embedder contract for permission and persistent push subscriptions.
pub trait PushProvider: Send + Sync {
    /// Returns `prompt`, `granted`, or `denied` for this owner and options.
    fn permission_state(&self, owner: &PushOwner, options: &PushSubscriptionOptionsData) -> String;
    /// Creates or returns the owner's active subscription.
    fn subscribe(
        &self,
        owner: PushOwner,
        options: PushSubscriptionOptionsData,
    ) -> Result<PushSubscriptionData, PushError>;
    /// Returns the active subscription, if any.
    fn get_subscription(&self, owner: &PushOwner) -> Option<PushSubscriptionData>;
    /// Removes a matching active subscription.
    fn unsubscribe(&self, owner: &PushOwner, endpoint: &str) -> Result<bool, PushError>;
    /// Removes state owned by an unregistered Service Worker scope.
    fn retire(&self, _owner: &PushOwner) {}
}

/// Deterministic push service for embedders and tests.
#[derive(Debug)]
pub struct DeterministicPushProvider {
    permission: Mutex<String>,
    supported: Mutex<bool>,
    subscriptions: Mutex<HashMap<(String, String, Option<u64>), PushSubscriptionData>>,
    next_endpoint: Mutex<u64>,
    next_error: Mutex<Option<PushError>>,
}

impl Default for DeterministicPushProvider {
    fn default() -> Self {
        Self {
            permission: Mutex::new("prompt".into()),
            supported: Mutex::new(false),
            subscriptions: Mutex::new(HashMap::new()),
            next_endpoint: Mutex::new(1),
            next_error: Mutex::new(None),
        }
    }
}

impl DeterministicPushProvider {
    /// Enables a deterministic local push service.
    pub fn set_supported(&self, supported: bool) {
        *self.supported.lock().unwrap() = supported;
    }

    /// Sets the permission reported to future calls.
    pub fn set_permission(&self, permission: &str) {
        *self.permission.lock().unwrap() = permission.to_owned();
    }

    /// Makes the next mutating provider call fail.
    pub fn fail_next(&self, error: PushError) {
        *self.next_error.lock().unwrap() = Some(error);
    }

    /// Returns all current subscriptions keyed by origin and scope.
    pub fn subscriptions(&self) -> Vec<PushSubscriptionData> {
        self.subscriptions
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect()
    }
}

fn owner_key(owner: &PushOwner) -> (String, String, Option<u64>) {
    (
        owner.origin.clone(),
        owner.scope.clone(),
        owner.registration_id,
    )
}

impl PushProvider for DeterministicPushProvider {
    fn permission_state(
        &self,
        _owner: &PushOwner,
        _options: &PushSubscriptionOptionsData,
    ) -> String {
        self.permission.lock().unwrap().clone()
    }

    fn subscribe(
        &self,
        owner: PushOwner,
        options: PushSubscriptionOptionsData,
    ) -> Result<PushSubscriptionData, PushError> {
        if let Some(error) = self.next_error.lock().unwrap().take() {
            return Err(error);
        }
        if !*self.supported.lock().unwrap() {
            return Err(PushError {
                name: "NotSupportedError".into(),
                message: "No host push service is installed".into(),
            });
        }
        if self.permission_state(&owner, &options) != "granted" {
            return Err(PushError {
                name: "NotAllowedError".into(),
                message: "Push permission is not granted".into(),
            });
        }
        let key = owner_key(&owner);
        let mut subscriptions = self.subscriptions.lock().unwrap();
        if let Some(current) = subscriptions.get(&key) {
            if current.options == options {
                return Ok(current.clone());
            }
            return Err(PushError {
                name: "InvalidStateError".into(),
                message: "An incompatible subscription already exists".into(),
            });
        }
        let mut next = self.next_endpoint.lock().unwrap();
        let subscription = PushSubscriptionData {
            endpoint: format!("https://push.invalid/subscription/{next}"),
            expiration_time: None,
            options,
            p256dh: (1..=65).collect(),
            auth: (201..=216).collect(),
        };
        *next = next.saturating_add(1);
        subscriptions.insert(key, subscription.clone());
        Ok(subscription)
    }

    fn get_subscription(&self, owner: &PushOwner) -> Option<PushSubscriptionData> {
        self.subscriptions
            .lock()
            .unwrap()
            .get(&owner_key(owner))
            .cloned()
    }

    fn unsubscribe(&self, owner: &PushOwner, endpoint: &str) -> Result<bool, PushError> {
        if let Some(error) = self.next_error.lock().unwrap().take() {
            return Err(error);
        }
        let key = owner_key(owner);
        let mut subscriptions = self.subscriptions.lock().unwrap();
        if subscriptions
            .get(&key)
            .is_some_and(|subscription| subscription.endpoint == endpoint)
        {
            subscriptions.remove(&key);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn retire(&self, owner: &PushOwner) {
        self.subscriptions.lock().unwrap().remove(&owner_key(owner));
    }
}

pub(super) struct State {
    pub(super) provider: Arc<dyn PushProvider>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            provider: Arc::new(DeterministicPushProvider::default()),
        }
    }
}

fn error_json(error: PushError) -> serde_json::Value {
    let name = match error.name.as_str() {
        "AbortError" | "InvalidAccessError" | "InvalidStateError" | "NotAllowedError"
        | "NotSupportedError" | "OperationError" => error.name,
        _ => "OperationError".to_owned(),
    };
    serde_json::json!({"error":name,"message":error.message})
}

fn validate_permission(permission: String) -> Result<String, PushError> {
    if matches!(permission.as_str(), "prompt" | "granted" | "denied") {
        Ok(permission)
    } else {
        Err(PushError {
            name: "OperationError".into(),
            message: "The push provider returned an invalid permission state".into(),
        })
    }
}

fn validate_subscription(
    subscription: PushSubscriptionData,
) -> Result<PushSubscriptionData, PushError> {
    let endpoint_is_https = url::Url::parse(&subscription.endpoint)
        .is_ok_and(|endpoint| endpoint.scheme() == "https" && endpoint.host().is_some());
    let expiration_is_valid = subscription
        .expiration_time
        .is_none_or(|expiration| expiration.is_finite());
    let application_key_is_valid = subscription
        .options
        .application_server_key
        .as_ref()
        .is_none_or(|key| key.len() == 65 && key.first() == Some(&4));
    if endpoint_is_https
        && expiration_is_valid
        && application_key_is_valid
        && subscription.p256dh.len() == 65
        && subscription.auth.len() >= 16
    {
        Ok(subscription)
    } else {
        Err(PushError {
            name: "OperationError".into(),
            message: "The push provider returned an invalid subscription".into(),
        })
    }
}

fn resolve_owner(
    host: &HostState,
    document_id: usize,
    scope: &str,
) -> Result<PushOwner, serde_json::Value> {
    let base = host.base_url_for_document(document_id).ok_or_else(
        || serde_json::json!({"error":"SecurityError","message":"Push requires a tuple origin"}),
    )?;
    let base = url::Url::parse(&base.to_string()).map_err(
        |_| serde_json::json!({"error":"SecurityError","message":"Push origin is unavailable"}),
    )?;
    let window_owner = scope.is_empty();
    let scope = if window_owner {
        base.join("/").expect("HTTP origin joins its root")
    } else {
        url::Url::parse(scope).map_err(|_| {
            serde_json::json!({"error":"InvalidStateError","message":"The registration scope is invalid"})
        })?
    };
    if scope.origin() != base.origin() {
        return Err(
            serde_json::json!({"error":"SecurityError","message":"Push scope must be same-origin"}),
        );
    }
    let top = host
        .worker_service_worker_top_level_site
        .clone()
        .or_else(|| host.storage_access_top_level_site(document_id))
        .unwrap_or_else(|| {
            crate::http::SchemefulSite::from_url(&scope.to_string().parse().unwrap())
        });
    let registration_id = host.worker_service_worker_registration_id.or_else(|| {
        host.cookie_store
            .lock()
            .unwrap()
            .service_worker_registration_id(scope.as_str(), &top)
    });
    if !window_owner && registration_id.is_none() {
        return Err(
            serde_json::json!({"error":"InvalidStateError","message":"The Service Worker registration is unavailable"}),
        );
    }
    Ok(PushOwner {
        origin: base.origin().ascii_serialization(),
        scope: scope.to_string(),
        registration_id,
    })
}

fn operation_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let operation = string_argument(args.first(), "", context)?;
    let document_id = parse_node_id(args.get(1), context)?;
    ensure_same_origin_document(context, document_id)?;
    let scope = string_argument(args.get(2), "", context)?;
    let json = string_argument(args.get(3), "{}", context)?;
    let result = with_host_state(|host| {
        let host = host.borrow();
        let owner = match resolve_owner(&host, document_id, &scope) {
            Ok(owner) => owner,
            Err(error) => return Ok(error),
        };
        match operation.as_str() {
            "permission" => {
                let options: PushSubscriptionOptionsData = serde_json::from_str(&json)
                    .map_err(|_| JsNativeError::typ().with_message("Invalid push options"))?;
                let notification = match host.notification_permission.as_str() {
                    "default" => "prompt",
                    value => value,
                };
                let permission = if notification == "granted" {
                    validate_permission(host.push_api.provider.permission_state(&owner, &options))
                } else {
                    Ok(notification.to_owned())
                };
                Ok(match permission {
                    Ok(permission) => serde_json::json!({"permission":permission}),
                    Err(error) => error_json(error),
                })
            }
            "subscribe" => {
                let options: PushSubscriptionOptionsData = serde_json::from_str(&json)
                    .map_err(|_| JsNativeError::typ().with_message("Invalid push options"))?;
                if host.notification_permission != "granted" {
                    return Ok(error_json(PushError {
                        name: "NotAllowedError".into(),
                        message: "Push permission is not granted".into(),
                    }));
                }
                match host
                    .push_api
                    .provider
                    .subscribe(owner, options)
                    .and_then(validate_subscription)
                {
                    Ok(subscription) => Ok(serde_json::to_value(subscription).unwrap()),
                    Err(error) => Ok(error_json(error)),
                }
            }
            "get" => Ok(match host.push_api.provider.get_subscription(&owner) {
                Some(subscription) => match validate_subscription(subscription) {
                    Ok(subscription) => serde_json::json!({"subscription":subscription}),
                    Err(error) => error_json(error),
                },
                None => serde_json::json!({"subscription":null}),
            }),
            "unsubscribe" => {
                let endpoint = serde_json::from_str::<serde_json::Value>(&json)
                    .ok()
                    .and_then(|value| value.get("endpoint")?.as_str().map(str::to_owned))
                    .unwrap_or_default();
                match host.push_api.provider.unsubscribe(&owner, &endpoint) {
                    Ok(removed) => Ok(serde_json::json!({"removed":removed})),
                    Err(error) => Ok(error_json(error)),
                }
            }
            "retire" => {
                host.push_api.provider.retire(&owner);
                Ok(serde_json::json!({"ok":true}))
            }
            _ => Err(JsNativeError::typ()
                .with_message("Unknown Push operation")
                .into()),
        }
    })?;
    Ok(JsValue::from(JsString::from(result.to_string().as_str())))
}

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_push"),
        4,
        NativeFunction::from_copy_closure(operation_native),
    )
}

impl JsRuntime {
    /// Replaces the push service shared by this runtime and subsequently created workers.
    pub fn set_push_provider(&mut self, provider: Arc<dyn PushProvider>) {
        self.host_state.borrow_mut().push_api.provider = provider;
    }

    /// Delivers an owned payload to the active Service Worker for `scope`.
    pub fn deliver_push_message(&mut self, scope: &str, payload: Option<&[u8]>) -> JsResult<bool> {
        self.dispatch_to_push_worker(scope, "push", &payload.map(|bytes| bytes.to_vec()))
    }

    /// Delivers a trusted subscription replacement/loss event to the owning worker.
    pub fn deliver_push_subscription_change(
        &mut self,
        scope: &str,
        new_subscription: Option<&PushSubscriptionData>,
        old_subscription: Option<&PushSubscriptionData>,
    ) -> JsResult<bool> {
        self.dispatch_to_push_worker(
            scope,
            "pushsubscriptionchange",
            &serde_json::json!({
                "newSubscription": new_subscription,
                "oldSubscription": old_subscription,
            }),
        )
    }

    fn dispatch_to_push_worker<T: Serialize>(
        &mut self,
        scope: &str,
        event: &str,
        payload: &T,
    ) -> JsResult<bool> {
        let workers = self
            .host_state
            .borrow()
            .workers
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for entry in workers {
            let mut worker = entry.borrow_mut();
            let matches = {
                let state = worker.runtime.host_state.borrow();
                !worker.terminated
                    && !state.worker_terminated
                    && state.worker_service_worker_scope.as_deref() == Some(scope)
                    && state.worker_service_worker_registration_id.is_some()
            };
            if !matches {
                continue;
            }
            let payload = serde_json::to_value(payload).map_err(|error| {
                JsNativeError::error()
                    .with_message(format!("Push payload serialization failed: {error}"))
            })?;
            super::service_worker::dispatch_owned_lifecycle_payload(
                &mut worker.runtime,
                event,
                &payload,
            )?;
            worker.runtime.run_until_idle()?;
            return Ok(true);
        }
        Ok(false)
    }
}
