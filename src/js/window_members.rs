//! Browsing-context values behind Window, Document, Navigator and Location
//! members (Issue #1291).
//!
//! Each native reads the explicit host state for one Document and never
//! mutates it. Value conversions are kept in pure helpers so tests can pass
//! their inputs directly.

use super::*;
use boa_engine::native_function::NativeFunctionPointer;
use std::time::UNIX_EPOCH;

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_property(
        context,
        bindings,
        js_string!("__omoikane_navigator_hardware_concurrency"),
        hardware_concurrency(),
        boa_engine::property::Attribute::all(),
    )?;
    let natives: [(JsString, usize, NativeFunctionPointer); 5] = [
        (
            js_string!("__omoikane_webdriver_active"),
            0,
            webdriver_active_native,
        ),
        (
            js_string!("__omoikane_document_last_modified"),
            1,
            document_last_modified_native,
        ),
        (
            js_string!("__omoikane_document_origin"),
            1,
            document_origin_native,
        ),
        (
            js_string!("__omoikane_document_domain"),
            2,
            document_domain_native,
        ),
        (
            js_string!("__omoikane_ancestor_origins"),
            1,
            ancestor_origins_native,
        ),
    ];
    for (name, length, function) in natives {
        register_private_callable(
            context,
            bindings,
            name,
            length,
            NativeFunction::from_fn_ptr(function),
        )?;
    }
    Ok(())
}

/// Logical processors reported by `navigator.hardwareConcurrency`, at least 1.
fn hardware_concurrency() -> u32 {
    std::thread::available_parallelism()
        .map_or(1, |count| u32::try_from(count.get()).unwrap_or(u32::MAX))
}

/// Parses a response's `Last-Modified` field as milliseconds since the Unix
/// epoch. A missing or unparsable field yields `None`, which makes
/// `document.lastModified` report the current time as HTML requires.
pub(super) fn response_last_modified(headers: &[(String, String)]) -> Option<f64> {
    let (_, value) = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("last-modified"))?;
    let time = crate::http::parse_http_date(value)?;
    Some(match time.duration_since(UNIX_EPOCH) {
        Ok(after) => after.as_millis() as f64,
        Err(before) => -(before.duration().as_millis() as f64),
    })
}

/// The serialized origin and, for a tuple origin, its host (the effective
/// domain HTML uses for `document.domain`).
fn document_origin_parts(state: &HostState, document_id: usize) -> (String, Option<String>) {
    let origin = match state.document_security_origins.get(&document_id) {
        Some(DocumentSecurityOrigin::Tuple(origin)) => Some(origin),
        Some(DocumentSecurityOrigin::Opaque(_)) => None,
        None => state
            .document_origins
            .get(&document_id)
            .and_then(Option::as_ref),
    };
    match origin {
        Some(origin) => (
            serialized_window_origin(Some(origin)),
            Some(origin.host().to_owned()),
        ),
        None => ("null".to_owned(), None),
    }
}

/// HTML "is a registrable domain suffix of or is equal to": `suffix` must be
/// `original` itself or a dot-separated suffix of it that is not a public
/// suffix. IP addresses only match themselves.
fn is_registrable_domain_suffix_or_equal(suffix: &str, original: &str) -> bool {
    if suffix.is_empty() {
        return false;
    }
    let Ok(host) = url::Host::parse(suffix) else {
        return false;
    };
    let Ok(original) = url::Host::parse(original) else {
        return false;
    };
    if host == original {
        return true;
    }
    let (url::Host::Domain(host), url::Host::Domain(original)) = (host, original) else {
        return false;
    };
    original
        .strip_suffix(host.as_str())
        .is_some_and(|prefix| prefix.ends_with('.'))
        && !crate::http::is_public_suffix(&host)
}

/// Origins of the Documents containing `document_id`, nearest first. A
/// container whose `referrerpolicy` is `no-referrer` contributes `"null"`.
fn ancestor_origins(state: &HostState, mut document_id: usize) -> Vec<String> {
    let mut origins = Vec::new();
    for _ in 0..=state.iframe_documents.len() {
        let Some((frame_id, _)) = state
            .iframe_documents
            .iter()
            .find(|(_, entry)| entry.document.identity() == document_id)
        else {
            break;
        };
        let Some(frame) = state.get_node(*frame_id) else {
            break;
        };
        let Some(owner) = owner_document_for_node(&frame) else {
            break;
        };
        let hides_origin = frame
            .get_attribute("referrerpolicy")
            .is_some_and(|policy| policy.trim().eq_ignore_ascii_case("no-referrer"));
        origins.push(if hides_origin {
            "null".to_owned()
        } else {
            document_origin_parts(state, owner.identity()).0
        });
        document_id = owner.identity();
    }
    origins
}

/// `__omoikane_webdriver_active()` — whether host automation controls this
/// runtime, which HTML exposes as `navigator.webdriver`.
fn webdriver_active_native(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    with_host_state(|state| {
        Ok(JsValue::from(
            state.borrow().input_bridge.automation.enabled(),
        ))
    })
}

/// `__omoikane_document_last_modified(documentId)` — the response's
/// `Last-Modified` time in epoch milliseconds, or `null` when unknown.
fn document_last_modified_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, document_id)?;
    with_host_state(|state| {
        Ok(state
            .borrow()
            .document_reporting
            .get(&document_id)
            .and_then(|metadata| metadata.last_modified)
            .map_or_else(JsValue::null, JsValue::from))
    })
}

/// `__omoikane_document_origin(documentId)` — the serialized origin used by
/// `self.origin`.
fn document_origin_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, document_id)?;
    with_host_state(|state| {
        let (origin, _) = document_origin_parts(&state.borrow(), document_id);
        Ok(JsValue::from(js_string!(origin.as_str())))
    })
}

/// `__omoikane_document_domain(documentId[, value])` — with one argument,
/// returns the effective domain (`""` for an opaque origin). With a value,
/// returns whether HTML permits setting it. Omoikane keeps every agent
/// cluster origin-keyed, so a permitted value leaves the origin and every
/// same-origin check unchanged.
fn document_domain_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, document_id)?;
    let requested = match args.get(1) {
        Some(value) if !value.is_undefined() => {
            Some(value.to_string(context)?.to_std_string_escaped())
        }
        _ => None,
    };
    with_host_state(|state| {
        let (_, domain) = document_origin_parts(&state.borrow(), document_id);
        Ok(match requested {
            None => JsValue::from(js_string!(domain.unwrap_or_default().as_str())),
            Some(value) => JsValue::from(
                domain.is_some_and(|domain| is_registrable_domain_suffix_or_equal(&value, &domain)),
            ),
        })
    })
}

/// `__omoikane_ancestor_origins(documentId)` — the list backing
/// `location.ancestorOrigins`.
fn ancestor_origins_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, document_id)?;
    let origins = with_host_state(|state| Ok(ancestor_origins(&state.borrow(), document_id)))?;
    Ok(JsArray::from_iter(
        origins
            .iter()
            .map(|origin| JsValue::from(js_string!(origin.as_str()))),
        context,
    )
    .into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_modified_parses_imf_fixdate_and_ignores_invalid_values() {
        let header = |value: &str| vec![("Last-Modified".to_owned(), value.to_owned())];
        assert_eq!(
            response_last_modified(&header("Wed, 21 Oct 2015 07:28:00 GMT")),
            Some(1_445_412_480_000.0)
        );
        assert_eq!(response_last_modified(&header("not a date")), None);
        assert_eq!(response_last_modified(&[]), None);
    }

    #[test]
    fn registrable_domain_suffix_rejects_public_suffixes_and_partial_labels() {
        assert!(is_registrable_domain_suffix_or_equal(
            "example.com",
            "a.b.example.com"
        ));
        assert!(is_registrable_domain_suffix_or_equal(
            "EXAMPLE.com",
            "example.com"
        ));
        assert!(is_registrable_domain_suffix_or_equal(
            "b.example.com",
            "a.b.example.com"
        ));
        assert!(!is_registrable_domain_suffix_or_equal("com", "example.com"));
        assert!(!is_registrable_domain_suffix_or_equal(
            "ample.com",
            "example.com"
        ));
        assert!(!is_registrable_domain_suffix_or_equal(
            "other.com",
            "example.com"
        ));
        assert!(!is_registrable_domain_suffix_or_equal("", "example.com"));
        assert!(!is_registrable_domain_suffix_or_equal("0.1", "127.0.0.1"));
        assert!(is_registrable_domain_suffix_or_equal(
            "127.0.0.1",
            "127.0.0.1"
        ));
    }
}
