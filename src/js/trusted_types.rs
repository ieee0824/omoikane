//! Trusted Types values and CSP checks shared by same-origin document realms.

use super::*;
use boa_engine::JsData;

/// Native object data is the brand. A prototype change, `Object.create`, or
/// a value from another realm cannot forge or erase it.
#[derive(Trace, Finalize, JsData)]
struct TrustedTypeData {
    kind: u8,
    value: JsString,
}

fn type_kind(value: Option<&JsValue>) -> JsResult<u8> {
    match value.and_then(JsValue::as_number) {
        Some(0.0) => Ok(0),
        Some(1.0) => Ok(1),
        Some(2.0) => Ok(2),
        _ => Err(JsNativeError::typ()
            .with_message("invalid Trusted Type kind")
            .into()),
    }
}

pub(super) fn create_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let kind = type_kind(args.first())?;
    let value = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?;
    let prototype = args
        .get(2)
        .and_then(JsValue::as_object)
        .cloned()
        .ok_or_else(|| JsNativeError::typ().with_message("Trusted Type prototype is required"))?;
    Ok(JsObject::from_proto_and_data(Some(prototype), TrustedTypeData { kind, value }).into())
}

pub(super) fn is_native(_: &JsValue, args: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    let kind = type_kind(args.get(1))?;
    let matches = args
        .first()
        .and_then(JsValue::as_object)
        .and_then(JsObject::downcast_ref::<TrustedTypeData>)
        .is_some_and(|data| data.kind == kind);
    Ok(JsValue::from(matches))
}

pub(super) fn value_native(_: &JsValue, args: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    let kind = type_kind(args.get(1))?;
    let value = args
        .first()
        .and_then(JsValue::as_object)
        .and_then(JsObject::downcast_ref::<TrustedTypeData>)
        .filter(|data| data.kind == kind)
        .map(|data| data.value.clone())
        .ok_or_else(|| JsNativeError::typ().with_message("incompatible Trusted Type receiver"))?;
    Ok(value.into())
}

fn owning_document(state: &HostState, node_id: usize) -> JsResult<NodeHandle> {
    let node = state
        .get_node(node_id)
        .ok_or_else(|| JsNativeError::typ().with_message("Trusted Types target not found"))?;
    document_root_for_node(&node)
        .or_else(|| state.node_lifetime_owner(node_id))
        .ok_or_else(|| {
            JsNativeError::reference()
                .with_message("Trusted Types target has no owning Document")
                .into()
        })
}

pub(super) fn policy_check_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let name = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?;
    let name = name.to_std_string_escaped();
    let already_created = args.get(1).is_some_and(JsValue::to_boolean);
    let source = super::security_reporting::capture_csp_source(context);
    with_host_state(|host| {
        let (document_id, blocking, reporting) = {
            let state = host.borrow();
            let document = state.csp_document_for_context(context)?;
            let (blocking, reporting) = state
                .csp_policy_for_document(&document)
                .trusted_type_policy_creation_violations(&name, already_created);
            (document.identity(), blocking, reporting)
        };
        let allowed = blocking.is_empty();
        let sample = csp::inline_report_sample(&name);
        let mut state = host.borrow_mut();
        state.queue_trusted_types_violation(
            document_id,
            "trusted-types",
            "trusted-types-policy",
            blocking,
            "enforce",
            &sample,
            source.as_ref(),
        );
        state.queue_trusted_types_violation(
            document_id,
            "trusted-types",
            "trusted-types-policy",
            reporting,
            "report",
            &sample,
            source.as_ref(),
        );
        Ok(JsValue::from(allowed))
    })
}

pub(super) fn register_default_html_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let callback = args.first().cloned().unwrap_or_default();
    if !callback.is_null_or_undefined() && callback.as_callable().is_none() {
        return Err(JsNativeError::typ()
            .with_message("default Trusted Types callback must be callable")
            .into());
    }
    with_host_state(|host| {
        let document_id = host.borrow().csp_document_for_context(context)?.identity();
        host.borrow_mut()
            .trusted_type_default_html_callbacks
            .insert(document_id, callback);
        Ok(JsValue::undefined())
    })
}

pub(super) fn default_html_for_node_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    with_host_state(|host| {
        let state = host.borrow();
        let document_id = owning_document(&state, node_id)?.identity();
        Ok(state
            .trusted_type_default_html_callbacks
            .get(&document_id)
            .cloned()
            .unwrap_or_else(JsValue::null))
    })
}

pub(super) fn sink_requires_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    with_host_state(|host| {
        let state = host.borrow();
        let document = owning_document(&state, node_id)?;
        let (blocking, reporting) = state
            .csp_policy_for_document(&document)
            .trusted_type_sink_violations();
        Ok(JsValue::from(!blocking.is_empty() || !reporting.is_empty()))
    })
}

pub(super) fn sink_violation_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    let source_text = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?;
    let source = super::security_reporting::capture_csp_source(context);
    with_host_state(|host| {
        let (document_id, blocking, reporting) = {
            let state = host.borrow();
            let document = owning_document(&state, node_id)?;
            let (blocking, reporting) = state
                .csp_policy_for_document(&document)
                .trusted_type_sink_violations();
            (document.identity(), blocking, reporting)
        };
        let allowed = blocking.is_empty();
        let sample = format!(
            "Element innerHTML|{}",
            csp::inline_report_sample(&source_text.to_std_string_escaped())
        );
        let mut state = host.borrow_mut();
        state.queue_trusted_types_violation(
            document_id,
            "require-trusted-types-for",
            "trusted-types-sink",
            blocking,
            "enforce",
            &sample,
            source.as_ref(),
        );
        state.queue_trusted_types_violation(
            document_id,
            "require-trusted-types-for",
            "trusted-types-sink",
            reporting,
            "report",
            &sample,
            source.as_ref(),
        );
        Ok(JsValue::from(allowed))
    })
}
