//! Read-only Cookie Store queries backed by the shared HTTP cookie jar.

use super::*;

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
        let site = state
            .location_href
            .parse::<crate::http::Url>()
            .unwrap_or_else(|_| url.clone());
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
