//! Cache Storage: the JSON boundary between the page wrappers and the host
//! storage manager.

use super::*;

/// Host-side backing store for the Cache Storage JavaScript wrappers.
///
/// Cache operations are deliberately exposed as one JSON boundary: request
/// and response objects are realm-local, immutable snapshots and must never be
/// retained as `JsValue`s by the native manager.  The wrapper queues the call
/// on the networking task source before invoking this binding, so the native
/// store itself remains synchronous and lock-scoped.
fn cache_storage_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let operation = string_argument(args.first(), "", context)?;
    let name = string_argument(args.get(1), "", context)?;
    let payload = string_argument(args.get(2), "", context)?;
    with_host_state(|host| {
        let state = host.borrow();
        let origin = state
            .document_origins
            .get(&state.document.identity())
            .cloned()
            .flatten()
            .ok_or_else(|| {
                JsError::from(
                    JsNativeError::error()
                        .with_message("Cache Storage is unavailable for an opaque origin"),
                )
            })?;
        let manager = state.storage_manager.clone();
        let output = match operation.as_str() {
            "open" => {
                manager.cache_open(&origin, name);
                serde_json::json!(true)
            }
            "has" => serde_json::json!(manager.cache_has(&origin, &name)),
            "keys" => serde_json::json!(manager.cache_names(&origin)),
            "delete" => serde_json::json!(manager.cache_delete(&origin, &name)),
            "entries" => {
                let entries = manager.cache_entries(&origin, &name).ok_or_else(|| {
                    JsError::from(
                        JsNativeError::error().with_message("Cache object no longer exists"),
                    )
                })?;
                serde_json::Value::Array(
                    entries
                        .into_iter()
                        .map(|entry| {
                            serde_json::json!({
                                "id": entry.id,
                                "request": entry.request,
                                "response": entry.response,
                            })
                        })
                        .collect(),
                )
            }
            "put" => {
                let value: serde_json::Value = serde_json::from_str(&payload).map_err(|error| {
                    JsError::from(
                        JsNativeError::typ()
                            .with_message(format!("invalid Cache.put snapshot: {error}")),
                    )
                })?;
                let request = value
                    .get("request")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        JsError::from(
                            JsNativeError::typ().with_message("Cache.put request snapshot missing"),
                        )
                    })?;
                let response = value
                    .get("response")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        JsError::from(
                            JsNativeError::typ()
                                .with_message("Cache.put response snapshot missing"),
                        )
                    })?;
                if manager
                    .cache_put(&origin, &name, request.to_string(), response.to_string())
                    .is_none()
                {
                    return Err(JsError::from(
                        JsNativeError::error().with_message("Cache object no longer exists"),
                    ));
                }
                serde_json::json!(true)
            }
            "delete-entry" => {
                let id = serde_json::from_str::<u64>(&payload).map_err(|error| {
                    JsError::from(
                        JsNativeError::typ()
                            .with_message(format!("invalid Cache entry id: {error}")),
                    )
                })?;
                serde_json::json!(manager.cache_delete_entry(&origin, &name, id))
            }
            _ => {
                return Err(JsError::from(JsNativeError::typ().with_message(format!(
                    "unknown Cache Storage operation: {operation}"
                ))));
            }
        };
        let encoded = serde_json::to_string(&output).map_err(|error| {
            JsError::from(JsNativeError::error().with_message(error.to_string()))
        })?;
        Ok(js_string!(encoded).into())
    })
}

/// Registers the private Cache Storage hook used by the DOM bootstrap.
pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    for (name, length, function) in [(
        js_string!("__omoikane_cache_storage"),
        3,
        NativeFunction::from_copy_closure(cache_storage_native),
    )] {
        register_private_builtin_callable(context, bindings, name, length, function)?;
    }
    Ok(())
}
