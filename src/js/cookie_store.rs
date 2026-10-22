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
}

#[derive(serde::Deserialize)]
struct SubscriptionEntry {
    name: Option<String>,
    url: String,
}

/// Reads or updates cookie subscriptions for a same-origin Service Worker registration.
pub(super) fn subscriptions_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, document_id)?;
    let scope = string_argument(args.get(1), "", context)?;
    let operation = string_argument(args.get(2), "", context)?;
    let entries = string_argument(args.get(3), "[]", context)?;
    let expected_id = string_argument(args.get(4), "", context)?
        .parse::<u64>()
        .ok();
    let entries: Vec<SubscriptionEntry> = serde_json::from_str(&entries)
        .map_err(|_| JsNativeError::typ().with_message("Invalid cookie subscriptions"))?;
    let scope_url = url::Url::parse(&scope).map_err(|_| {
        JsNativeError::typ().with_message("Invalid Service Worker registration scope")
    })?;
    if entries.iter().any(|entry| {
        url::Url::parse(&entry.url)
            .map(|url| !url.as_str().starts_with(scope_url.as_str()))
            .unwrap_or(true)
    }) {
        return Err(JsNativeError::typ()
            .with_message("Cookie subscription URL is outside scope")
            .into());
    }
    with_host_state(|host| {
        let state = host.borrow();
        if !document_is_secure_context(&state, document_id) {
            return Err(JsNativeError::typ()
                .with_message("Cookie subscriptions require a secure context")
                .into());
        }
        let scope_origin = StorageOrigin::from_url(&scope);
        if scope_origin.is_none()
            || scope_origin.as_ref()
                != state
                    .document_origins
                    .get(&document_id)
                    .and_then(Option::as_ref)
            || state
                .worker_service_worker_scope
                .as_ref()
                .is_some_and(|worker_scope| worker_scope != &scope)
        {
            return Err(JsNativeError::typ()
                .with_message("Invalid Service Worker registration scope")
                .into());
        }
        let worker_scope = state.worker_service_worker_scope.is_some();
        let worker_registration_id = state.worker_service_worker_registration_id;
        let top_level_site = state
            .worker_service_worker_top_level_site
            .clone()
            .or_else(|| state.storage_access_top_level_site(document_id))
            .or_else(|| {
                scope
                    .parse::<crate::http::Url>()
                    .ok()
                    .map(|url| crate::http::SchemefulSite::from_url(&url))
            })
            .ok_or_else(|| JsNativeError::typ().with_message("Top-level site is unavailable"))?;
        let jar = state.cookie_store.clone();
        drop(state);
        let mut jar = jar.lock().unwrap();
        let current_id = jar.service_worker_registration_id(&scope, &top_level_site);
        if current_id.is_none()
            || (worker_scope && worker_registration_id != current_id)
            || (operation != "identity" && expected_id != current_id)
        {
            return Err(JsNativeError::typ()
                .with_message("Service Worker registration is unavailable")
                .into());
        }
        match operation.as_str() {
            "identity" => Ok(JsValue::from(js_string!(current_id.unwrap().to_string()))),
            "subscribe" | "unsubscribe" => {
                let entries = entries
                    .into_iter()
                    .map(|entry| crate::http::CookieStoreSubscription {
                        name: entry.name,
                        url: entry.url,
                    })
                    .collect();
                jar.update_service_worker_subscriptions(
                    &scope,
                    &top_level_site,
                    entries,
                    operation == "subscribe",
                );
                Ok(JsValue::undefined())
            }
            "get" => {
                let items = jar
                    .service_worker_subscriptions(&scope, &top_level_site)
                    .into_iter()
                    .map(|entry| match entry.name {
                        Some(name) => serde_json::json!({"name": name, "url": entry.url}),
                        None => serde_json::json!({"url": entry.url}),
                    })
                    .collect::<Vec<_>>();
                Ok(JsValue::from(js_string!(
                    serde_json::to_string(&items).expect("subscriptions are serializable")
                )))
            }
            "clear" => {
                if worker_scope {
                    return Err(JsNativeError::typ()
                        .with_message("Only the registration owner can unregister")
                        .into());
                }
                jar.clear_service_worker_subscriptions(&scope, &top_level_site);
                Ok(JsValue::undefined())
            }
            _ => Err(JsNativeError::typ()
                .with_message("Invalid cookie subscription operation")
                .into()),
        }
    })
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
        let access = state.cookie_access_context_for_document(
            document_id,
            cookie_access_allowed(&state, document_id),
        );
        let site = state.storage_access_cookie_site_for_document(document_id, &url);
        let pairs = state
            .cookie_store
            .lock()
            .unwrap()
            .script_visible_cookie_pairs_with_access(&url, &site, &access);
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
        let access = state.cookie_access_context_for_document(
            document_id,
            cookie_access_allowed(&state, document_id),
        );
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
        };
        state
            .cookie_store
            .lock()
            .unwrap()
            .set_from_cookie_store_with_access(options, &url, &site, &access)
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
        let access = state.cookie_access_context_for_document(
            document_id,
            cookie_access_allowed(&state, document_id),
        );
        let site = state.storage_access_cookie_site_for_document(document_id, &url);
        let jar = state.cookie_store.lock().unwrap();
        let (next, changes) = if let Some(cursor) = cursor {
            jar.script_visible_changes_since_with_access(cursor, &url, &site, &access)
        } else {
            (jar.change_sequence(), Vec::new())
        };
        Ok(encode_cookie_changes(next, changes))
    })
}

/// Reads changes matching the current Service Worker registration's subscriptions.
pub(super) fn subscribed_changes_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, document_id)?;
    let cursor = args
        .get(1)
        .and_then(JsValue::as_number)
        .map(|value| value as u64);
    with_host_state(|host| {
        let state = host.borrow();
        let scope = state.worker_service_worker_scope.as_ref().ok_or_else(|| {
            JsNativeError::typ()
                .with_message("Cookie change subscription requires a Service Worker")
        })?;
        let top_level_site = state
            .worker_service_worker_top_level_site
            .as_ref()
            .ok_or_else(|| JsNativeError::typ().with_message("Top-level site is unavailable"))?;
        let script_url = state.document_base_urls.get(&document_id).ok_or_else(|| {
            JsNativeError::typ().with_message("Service Worker URL is unavailable")
        })?;
        let jar = state.cookie_store.lock().unwrap();
        let next = jar.change_sequence();
        if state.worker_service_worker_registration_id.is_none()
            || state.worker_service_worker_registration_id
                != jar.service_worker_registration_id(scope, top_level_site)
        {
            return Ok(encode_cookie_changes(next, Vec::new()));
        }

        let unpartitioned = cookie_access_allowed(&state, document_id);
        let site = state.storage_access_cookie_site_for_document(document_id, script_url);
        let (next, changes) = if let Some(cursor) = cursor {
            jar.service_worker_changes_since_with_access(
                cursor,
                scope,
                top_level_site,
                &site,
                unpartitioned,
            )
        } else {
            (next, Vec::new())
        };
        Ok(encode_cookie_changes(next, changes))
    })
}

fn encode_cookie_changes(next: u64, changes: Vec<crate::http::CookieStoreChange>) -> JsValue {
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
    JsValue::from(js_string!(payload.to_string()))
}
