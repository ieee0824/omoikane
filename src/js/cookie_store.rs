//! Cookie Store operations backed by the shared HTTP cookie jar.

use super::*;
use std::time::{Duration, SystemTime};

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Mutation {
    name: String,
    value: String,
    domain: Option<String>,
    path: String,
    expires: Option<f64>,
    max_age: Option<i64>,
    same_site: String,
    partitioned: bool,
    deletion: bool,
}

fn expiry_time(milliseconds: f64) -> Option<SystemTime> {
    if !milliseconds.is_finite() || milliseconds.abs() > u64::MAX as f64 {
        return None;
    }
    let duration = Duration::from_millis(milliseconds.abs() as u64);
    if milliseconds < 0.0 {
        SystemTime::UNIX_EPOCH.checked_sub(duration)
    } else {
        SystemTime::UNIX_EPOCH.checked_add(duration)
    }
}

fn cookie_access_allowed(state: &HostState, document_id: usize) -> bool {
    // The synthetic document in a Worker has no parent browsing context. Use
    // the inherited request context so a cross-site owner cannot lend its
    // Document-only Storage Access grant to the ServiceWorker's CookieStore.
    if document_id == state.document.identity()
        && state
            .worker_cookie_request_context
            .as_ref()
            .is_some_and(crate::http::CookieRequestContext::has_cross_site_ancestry)
    {
        return false;
    }
    state.storage_access_allows_document_cookies(document_id)
}

/// Reports secure-context exposure for the calling document's CookieStore.
pub(super) fn available_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, document_id)?;
    with_host_state(|host| {
        let state = host.borrow();
        Ok(JsValue::from(document_is_secure_context(
            &state,
            document_id,
        )))
    })
}

/// Queries script-visible cookies for the document's creation URL.
///
/// A null result denotes an opaque origin; the JavaScript binding turns it
/// into a rejected `SecurityError` promise. All other results are JSON lists.
pub(super) fn query_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, document_id)?;
    let creation_url = args
        .get(1)
        .ok_or_else(|| JsNativeError::typ().with_message("Cookie Store URL is unavailable"))?
        .to_string(context)?
        .to_std_string_escaped();
    with_host_state(|host| {
        let state = host.borrow();
        if !document_is_secure_context(&state, document_id) {
            return Err(JsNativeError::typ()
                .with_message("Cookie Store requires a secure context")
                .into());
        }
        if !state
            .document_origins
            .get(&document_id)
            .is_some_and(Option::is_some)
        {
            return Ok(JsValue::null());
        }
        // Non-HTTP creation URLs (for example about:srcdoc) have no cookies.
        let Ok(url) = creation_url.parse::<crate::http::Url>() else {
            return Ok(JsValue::from(js_string!("[]")));
        };
        if state
            .document_origins
            .get(&document_id)
            .and_then(Option::as_ref)
            != StorageOrigin::from_url(&creation_url).as_ref()
        {
            return Err(JsNativeError::typ()
                .with_message("Cookie Store document origin changed")
                .into());
        }
        if !cookie_access_allowed(&state, document_id) {
            return Ok(JsValue::from(js_string!("[]")));
        }
        let site = state.storage_access_cookie_site_for_document(document_id, &url);
        let pairs = state
            .cookie_store
            .lock()
            .unwrap()
            .script_visible_cookie_pairs(&url, &site);
        let items = pairs
            .into_iter()
            .map(|(name, value)| serde_json::json!({ "name": name, "value": value }))
            .collect::<Vec<_>>();
        Ok(JsValue::from(js_string!(
            serde_json::to_string(&items).expect("cookie names and values are serializable")
        )))
    })
}

/// Applies a Cookie Store write through the HTTP jar's non-HTTP storage path.
/// A null result denotes an opaque origin and becomes a SecurityError promise
/// rejection in the JavaScript binding.
pub(super) fn mutate_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, document_id)?;
    let creation_url = args
        .get(1)
        .ok_or_else(|| JsNativeError::typ().with_message("Cookie Store URL is unavailable"))?
        .to_string(context)?
        .to_std_string_escaped();
    let encoded = args
        .get(2)
        .ok_or_else(|| JsNativeError::typ().with_message("Cookie Store options are required"))?
        .to_string(context)?
        .to_std_string_escaped();
    let mutation: Mutation = serde_json::from_str(&encoded)
        .map_err(|_| JsNativeError::typ().with_message("Invalid Cookie Store options"))?;
    let same_site = match mutation.same_site.as_str() {
        "strict" => crate::http::SameSite::Strict,
        "lax" => crate::http::SameSite::Lax,
        "none" => crate::http::SameSite::None,
        _ => {
            return Err(JsNativeError::typ()
                .with_message("Invalid Cookie Store sameSite")
                .into());
        }
    };
    let expires = mutation
        .expires
        .map(|millis| {
            expiry_time(millis)
                .ok_or_else(|| JsNativeError::typ().with_message("Invalid Cookie Store expires"))
        })
        .transpose()?;
    with_host_state(|host| {
        let state = host.borrow();
        if !document_is_secure_context(&state, document_id) {
            return Err(JsNativeError::typ()
                .with_message("Cookie Store requires a secure context")
                .into());
        }
        if !state
            .document_origins
            .get(&document_id)
            .is_some_and(Option::is_some)
        {
            return Ok(JsValue::null());
        }
        let Some(url) = creation_url.parse::<crate::http::Url>().ok() else {
            return Err(JsNativeError::typ()
                .with_message("Cookie Store document URL is unavailable")
                .into());
        };
        if state
            .document_origins
            .get(&document_id)
            .and_then(Option::as_ref)
            != StorageOrigin::from_url(&creation_url).as_ref()
        {
            return Err(JsNativeError::typ()
                .with_message("Cookie Store document origin changed")
                .into());
        }
        if !cookie_access_allowed(&state, document_id) {
            return Ok(JsValue::undefined());
        }
        let site = state.storage_access_cookie_site_for_document(document_id, &url);
        let options = crate::http::CookieStoreWrite {
            name: mutation.name,
            value: mutation.value,
            domain: mutation.domain,
            path: mutation.path,
            expires,
            max_age: mutation.max_age,
            same_site,
            partitioned: mutation.partitioned,
            deletion: mutation.deletion,
        };
        state
            .cookie_store
            .lock()
            .unwrap()
            .set_from_cookie_store(options, &url, &site)
            .map_err(|reason| JsNativeError::typ().with_message(reason))?;
        Ok(JsValue::undefined())
    })
}

/// Reads cookie changes for a Window CookieStore without exposing HttpOnly
/// values. Passing null as the cursor starts at the jar's current sequence.
pub(super) fn changes_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, document_id)?;
    let creation_url = args
        .get(1)
        .ok_or_else(|| JsNativeError::typ().with_message("Cookie Store URL is unavailable"))?
        .to_string(context)?
        .to_std_string_escaped();
    let cursor = args
        .get(2)
        .and_then(JsValue::as_number)
        .map(|value| value as u64);
    with_host_state(|host| {
        let state = host.borrow();
        if !document_is_secure_context(&state, document_id) {
            return Err(JsNativeError::typ()
                .with_message("Cookie Store requires a secure context")
                .into());
        }
        if !state
            .document_origins
            .get(&document_id)
            .is_some_and(Option::is_some)
        {
            return Ok(JsValue::null());
        }
        let Some(url) = creation_url.parse::<crate::http::Url>().ok() else {
            return Ok(JsValue::from(js_string!("{\"cursor\":0,\"events\":[]}")));
        };
        if state
            .document_origins
            .get(&document_id)
            .and_then(Option::as_ref)
            != StorageOrigin::from_url(&creation_url).as_ref()
        {
            return Err(JsNativeError::typ()
                .with_message("Cookie Store document origin changed")
                .into());
        }
        if !cookie_access_allowed(&state, document_id) {
            let next = state.cookie_store.lock().unwrap().change_sequence();
            return Ok(JsValue::from(js_string!(format!(
                "{{\"cursor\":{next},\"events\":[]}}"
            ))));
        }
        let site = state.storage_access_cookie_site_for_document(document_id, &url);
        let jar = state.cookie_store.lock().unwrap();
        let (next, changes) = if let Some(cursor) = cursor {
            jar.script_visible_changes_since(cursor, &url, &site)
        } else {
            (jar.change_sequence(), Vec::new())
        };
        let mut events = Vec::new();
        let mut batch_changed = Vec::new();
        let mut batch_deleted = Vec::new();
        for crate::http::CookieStoreChange {
            name,
            value,
            deleted: removed,
            individual_event,
        } in changes
        {
            if individual_event {
                if !batch_changed.is_empty() || !batch_deleted.is_empty() {
                    events.push(serde_json::json!({
                        "changed": std::mem::take(&mut batch_changed),
                        "deleted": std::mem::take(&mut batch_deleted),
                    }));
                }
                if removed {
                    events.push(serde_json::json!({
                        "changed": [], "deleted": [{ "name": name }],
                    }));
                } else {
                    events.push(serde_json::json!({
                        "changed": [{ "name": name, "value": value }], "deleted": [],
                    }));
                }
            } else if removed {
                batch_deleted.push(serde_json::json!({ "name": name }));
            } else {
                batch_changed.push(serde_json::json!({ "name": name, "value": value }));
            }
        }
        if !batch_changed.is_empty() || !batch_deleted.is_empty() {
            events.push(serde_json::json!({
                "changed": batch_changed,
                "deleted": batch_deleted,
            }));
        }
        let payload = serde_json::json!({ "cursor": next, "events": events });
        Ok(JsValue::from(js_string!(payload.to_string())))
    })
}
