//! Trusted Types values and CSP checks shared by same-origin document realms.

use super::*;
use boa_engine::JsData;
use sha2::{Digest, Sha256};

const MAX_TRUSTED_SCRIPT_TEXT_APPROVALS: usize = 1024;
const MAX_SCRIPT_SUBTREE_NODES: usize = 16_384;

/// One-use proof for a trusted script text assignment in a specific Document.
pub(super) struct ScriptTextApproval {
    pub(super) owner_document_id: usize,
    source_digest: [u8; 32],
    ancestry_digest: [u8; 32],
    root_id: usize,
    root_parent_revision: u64,
    root_was_document: bool,
}

/// A detour through a detached element invalidates earlier script text
/// approval. DocumentFragment staging stays valid because it moves the same
/// script and source directly into a live Document.
pub(super) fn invalidate_approvals_for_detached_element(
    state: &mut HostState,
    inserted: &NodeHandle,
) {
    if state.trusted_script_text_approvals.is_empty() {
        return;
    }
    for script in super::collect_script_elements(inserted) {
        state
            .trusted_script_text_approvals
            .remove(&script.identity());
    }
}

/// Native object data is the brand. A prototype change, `Object.create`, or
/// a value from another realm cannot forge or erase it.
#[derive(Trace, Finalize, JsData)]
struct TrustedTypeData {
    kind: u8,
    value: JsString,
}

pub(super) fn script_value(value: &JsValue) -> Option<JsString> {
    value
        .as_object()
        .as_ref()
        .and_then(JsObject::downcast_ref::<TrustedTypeData>)
        .filter(|data| data.kind == 1)
        .map(|data| data.value.clone())
}

/// Checks a dynamic script sink against the calling realm's CSP. The returned
/// source is the trusted value or the default policy's synchronous conversion.
pub(super) fn compliant_script_for_context(
    value: &JsValue,
    sink: &str,
    context: &mut Context,
) -> JsResult<JsString> {
    let realm = context.realm().clone();
    compliant_script_for_realm(value, sink, &realm, None, context)
}

fn compliant_script_for_realm(
    value: &JsValue,
    sink: &str,
    realm: &Realm,
    sample_source: Option<&str>,
    context: &mut Context,
) -> JsResult<JsString> {
    if let Some(source) = script_value(value) {
        return Ok(source);
    }
    let input = value.to_string(context)?;
    let source = super::security_reporting::capture_csp_source(context);
    let (document_id, blocking, reporting, callback) = with_host_state(|host| {
        let state = host.borrow();
        let (document_id, policy) = state.dynamic_code_policy_for_realm(realm)?;
        let (blocking, reporting) = policy.trusted_type_sink_violations();
        let callback = state
            .trusted_type_default_callbacks
            .get(&document_id)
            .map(|callbacks| callbacks[1].clone());
        Ok((document_id, blocking, reporting, callback))
    })?;
    if blocking.is_empty() && reporting.is_empty() {
        return Ok(input);
    }
    if let Some(callback) = callback.and_then(|value| value.as_callable()) {
        let converted = callback.call(
            &JsValue::undefined(),
            &[
                input.clone().into(),
                js_string!("TrustedScript").into(),
                js_string!(sink).into(),
            ],
            context,
        )?;
        if !converted.is_null_or_undefined() {
            return converted.to_string(context);
        }
    }
    let input_source = input.to_std_string_escaped();
    let sample = format!(
        "{sink}|{}",
        csp::inline_report_sample(sample_source.unwrap_or(&input_source))
    );
    with_host_state(|host| {
        let mut state = host.borrow_mut();
        state.queue_trusted_types_violation(
            document_id,
            "require-trusted-types-for",
            "trusted-types-sink",
            blocking.clone(),
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
        Ok(())
    })?;
    if !blocking.is_empty() {
        return Err(JsNativeError::typ()
            .with_message(format!("TrustedScript is required for {sink}"))
            .into());
    }
    Ok(input)
}

/// Applies CSP's dynamic-code brand and source-list checks before Boa parses
/// `eval` or a Function constructor. `code` is the exact generated source;
/// a default policy may approve it but cannot substitute other code.
pub(super) fn ensure_script_compilation(
    realm: &Realm,
    code: &JsString,
    trusted_script: bool,
    function_constructor: bool,
    context: &mut Context,
) -> JsResult<()> {
    let sink = if function_constructor {
        "Function"
    } else {
        "eval"
    };
    if !trusted_script {
        // Function constructor's policy callback receives the complete
        // generated source. CSP's violation sample starts at the formal
        // parameter list, omitting the synthesized function prefix.
        let sample_source = if function_constructor {
            let source = code.to_std_string_escaped();
            Some(
                source
                    .find('(')
                    .map_or(source.clone(), |index| source[index..].to_owned()),
            )
        } else {
            None
        };
        let checked = compliant_script_for_realm(
            &code.clone().into(),
            sink,
            realm,
            sample_source.as_deref(),
            context,
        )
        .map_err(|error| {
            if error
                .as_native()
                .is_some_and(JsNativeError::is_runtime_limit)
            {
                error
            } else {
                JsNativeError::eval()
                    .with_message("TrustedScript is required for dynamic code")
                    .into()
            }
        })?;
        if checked != *code {
            return Err(JsNativeError::eval()
                .with_message("Default policy changed dynamic code")
                .into());
        }
    }
    enforce_script_source_policy(realm, code, context)
}

/// Applies script-src/default-src dynamic compilation checks to eval,
/// Function constructors, and string-based timers after Trusted Types checks.
pub(super) fn enforce_script_source_policy(
    realm: &Realm,
    code: &JsString,
    context: &mut Context,
) -> JsResult<()> {
    let source = super::security_reporting::capture_csp_source(context);
    let (document_id, blocking, reporting) = with_host_state(|host| {
        let state = host.borrow();
        let (document_id, policy) = state.dynamic_code_policy_for_realm(realm)?;
        let (blocking, reporting) = policy.script_compilation_violations();
        Ok((document_id, blocking, reporting))
    })?;
    let sample = csp::inline_report_sample(&code.to_std_string_escaped());
    with_host_state(|host| {
        let mut state = host.borrow_mut();
        state.queue_trusted_types_violation(
            document_id,
            "script-src",
            "eval",
            blocking.clone(),
            "enforce",
            &sample,
            source.as_ref(),
        );
        state.queue_trusted_types_violation(
            document_id,
            "script-src",
            "eval",
            reporting,
            "report",
            &sample,
            source.as_ref(),
        );
        Ok(())
    })?;
    if !blocking.is_empty() {
        return Err(JsNativeError::eval()
            .with_message("Content Security Policy blocked dynamic code")
            .into());
    }
    Ok(())
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
        .ok_or_else(|| JsNativeError::typ().with_message("Trusted Type prototype is required"))?;
    Ok(JsObject::from_proto_and_data(Some(prototype), TrustedTypeData { kind, value }).into())
}

pub(super) fn is_native(_: &JsValue, args: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    let kind = type_kind(args.get(1))?;
    let matches = args
        .first()
        .and_then(JsValue::as_object)
        .as_ref()
        .and_then(JsObject::downcast_ref::<TrustedTypeData>)
        .is_some_and(|data| data.kind == kind);
    Ok(JsValue::from(matches))
}

pub(super) fn value_native(_: &JsValue, args: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    let kind = type_kind(args.get(1))?;
    let value = args
        .first()
        .and_then(JsValue::as_object)
        .as_ref()
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

/// Resolves the owning Document's policy for a DOM sink, including a retained
/// Document whose browsing context has navigated away. Inert Documents made
/// by DOMImplementation have no browsing context or CSP and use the default.
fn sink_policy_for_document(state: &HostState, document: &NodeHandle) -> CspPolicy {
    let document_id = document.identity();
    if state.document_is_active(document_id) {
        return state.csp_policy_for_document(document);
    }
    state
        .retired_document_csp
        .get(&document_id)
        .cloned()
        .unwrap_or_default()
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

pub(super) fn register_default_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let callbacks = std::array::from_fn(|index| args.get(index).cloned().unwrap_or_default());
    for callback in &callbacks {
        if !callback.is_null_or_undefined() && callback.as_callable().is_none() {
            return Err(JsNativeError::typ()
                .with_message("default Trusted Types callback must be callable")
                .into());
        }
    }
    with_host_state(|host| {
        let document_id = host.borrow().csp_document_for_context(context)?.identity();
        host.borrow_mut()
            .trusted_type_default_callbacks
            .insert(document_id, callbacks);
        Ok(JsValue::undefined())
    })
}

pub(super) fn default_for_node_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    let kind = type_kind(args.get(1))? as usize;
    with_host_state(|host| {
        let state = host.borrow();
        let document_id = owning_document(&state, node_id)?.identity();
        Ok(state
            .trusted_type_default_callbacks
            .get(&document_id)
            .map(|callbacks| callbacks[kind].clone())
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
        let (blocking, reporting) =
            sink_policy_for_document(&state, &document).trusted_type_sink_violations();
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
    let sink_name = args
        .get(2)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let source = super::security_reporting::capture_csp_source(context);
    with_host_state(|host| {
        let (document_id, blocking, reporting) = {
            let state = host.borrow();
            let document = owning_document(&state, node_id)?;
            let (blocking, reporting) =
                sink_policy_for_document(&state, &document).trusted_type_sink_violations();
            (document.identity(), blocking, reporting)
        };
        let allowed = blocking.is_empty();
        let sample = format!(
            "{sink_name}|{}",
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

fn script_text_digest(node: &NodeHandle, source: &str) -> Option<[u8; 32]> {
    let mut digest = Sha256::new();
    digest.update(source.len().to_le_bytes());
    digest.update(source.as_bytes());
    let mut nodes = vec![node.clone()];
    let mut count = 0;
    while let Some(current) = nodes.pop() {
        count += 1;
        if count > MAX_SCRIPT_SUBTREE_NODES {
            return None;
        }
        digest.update(current.identity().to_le_bytes());
        digest.update(current.mutation_revision().to_le_bytes());
        let mut child = current.first_child();
        while let Some(next) = child {
            if count + nodes.len() >= MAX_SCRIPT_SUBTREE_NODES {
                return None;
            }
            child = next.next_sibling();
            nodes.push(next);
        }
    }
    Some(digest.finalize().into())
}

fn script_ancestry_digest(
    node: &NodeHandle,
    root_id: Option<usize>,
) -> Option<([u8; 32], NodeHandle)> {
    let mut digest = Sha256::new();
    let mut current = node.clone();
    for _ in 0..MAX_SCRIPT_SUBTREE_NODES {
        digest.update(current.identity().to_le_bytes());
        if root_id == Some(current.identity()) || current.parent_node().is_none() {
            return Some((digest.finalize().into(), current));
        }
        digest.update(current.parent_revision().to_le_bytes());
        current = current.parent_node()?;
    }
    None
}

/// Remembers one source that passed the script textContent sink. No source or
/// JavaScript object is retained; a subtree mutation invalidates its digest.
pub(super) fn remember_script_text_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, id)?;
    let source = args
        .get(1)
        .and_then(JsValue::as_string)
        .ok_or_else(|| JsNativeError::typ().with_message("script text must be a string"))?
        .to_std_string_escaped();
    let approved = args.get(2).and_then(JsValue::as_boolean).unwrap_or(false);
    with_host_state(|host| {
        let mut state = host.borrow_mut();
        let node = state.get_node(id).ok_or_else(|| {
            JsNativeError::reference().with_message("script node no longer exists")
        })?;
        if !super::is_html_script_node(&node) {
            return Ok(JsValue::undefined());
        }
        state.trusted_script_text_approvals.remove(&id);
        if !approved {
            return Ok(JsValue::undefined());
        }
        let owner_id = owning_document(&state, id)?.identity();
        // When the cap is full, the setter still succeeds; later script
        // preparation checks the untrusted string and fails closed.
        if state.trusted_script_text_approvals.len() < MAX_TRUSTED_SCRIPT_TEXT_APPROVALS {
            if let (Some(source_digest), Some((ancestry_digest, root))) = (
                script_text_digest(&node, &source),
                script_ancestry_digest(&node, None),
            ) {
                state.trusted_script_text_approvals.insert(
                    id,
                    ScriptTextApproval {
                        owner_document_id: owner_id,
                        source_digest,
                        ancestry_digest,
                        root_id: root.identity(),
                        root_parent_revision: root.parent_revision(),
                        root_was_document: root.node_type() == NodeType::Document,
                    },
                );
            }
        }
        Ok(JsValue::undefined())
    })
}

/// Consumes a script text approval before dynamic execution. A move to another
/// Document, child mutation, or a changed string invalidates the approval.
pub(super) fn take_script_text_approval_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, id)?;
    let source = args
        .get(1)
        .and_then(JsValue::as_string)
        .ok_or_else(|| JsNativeError::typ().with_message("script text must be a string"))?
        .to_std_string_escaped();
    with_host_state(|host| {
        let mut state = host.borrow_mut();
        let approved = state.trusted_script_text_approvals.remove(&id);
        let matches = if let (Some(approval), Some(node)) = (approved, state.get_node(id)) {
            // A detached script may be inserted through a DocumentFragment,
            // which changes its parent link more than once before execution.
            // Its one-use approval follows the same node and exact subtree;
            // a script already in a Document still cannot be detached and
            // reconnected to reuse its earlier approval.
            let root_matches = if approval.root_was_document {
                script_ancestry_digest(&node, Some(approval.root_id)).is_some_and(
                    |(digest, root)| {
                        digest == approval.ancestry_digest
                            && root.identity() == approval.root_id
                            && root.parent_revision() == approval.root_parent_revision
                    },
                )
            } else {
                true
            };
            owning_document(&state, id)?.identity() == approval.owner_document_id
                && root_matches
                && script_text_digest(&node, &source)
                    .is_some_and(|actual| actual == approval.source_digest)
        } else {
            false
        };
        Ok(JsValue::from(matches))
    })
}

#[cfg(test)]
mod approval_tests {
    use super::*;

    #[test]
    fn script_text_approval_capacity_fails_closed() {
        let document = crate::html::TreeBuilder::parse("<body>").document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        runtime.install_response_security_headers(&[(
            "Content-Security-Policy".into(),
            "script-src 'unsafe-inline'; require-trusted-types-for 'script'".into(),
        )]);
        {
            let mut state = runtime.host_state.borrow_mut();
            let owner_document_id = state.document.identity();
            for index in 0..MAX_TRUSTED_SCRIPT_TEXT_APPROVALS {
                state.trusted_script_text_approvals.insert(
                    usize::MAX - index,
                    ScriptTextApproval {
                        owner_document_id,
                        source_digest: [0; 32],
                        ancestry_digest: [0; 32],
                        root_id: 0,
                        root_parent_revision: 0,
                        root_was_document: false,
                    },
                );
            }
        }
        let result = runtime
            .eval(
                "const policy = trustedTypes.createPolicy('cap', {createScript: s => s}); \
                 const script = document.createElement('script'); \
                 script.textContent = policy.createScript('globalThis.capacityExecuted = true'); \
                 document.body.appendChild(script); \
                 globalThis.capacityExecuted === undefined",
            )
            .unwrap();
        assert_eq!(result.as_boolean(), Some(true));
        assert_eq!(
            runtime
                .host_state
                .borrow()
                .trusted_script_text_approvals
                .len(),
            MAX_TRUSTED_SCRIPT_TEXT_APPROVALS
        );
    }
}
