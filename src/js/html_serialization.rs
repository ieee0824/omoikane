//! HTML fragment serialization with explicitly selected shadow trees.
use std::collections::HashSet;

use super::*;
use crate::dom::{Node, NodeHandle, NodeType, ShadowRootMode, ShadowRootSettings};

/// Owned UTF-16 output; markup is scalar text, DOM data may contain surrogates.
#[derive(Default)]
struct HtmlOutput(Vec<u16>);

impl HtmlOutput {
    fn push_str(&mut self, value: &str) {
        self.0.extend(value.encode_utf16());
    }

    fn push(&mut self, value: char) {
        let mut units = [0; 2];
        self.0.extend_from_slice(value.encode_utf16(&mut units));
    }

    fn attribute(&mut self, units: &[u16]) {
        for &unit in units {
            match unit {
                0x26 => self.push_str("&amp;"),
                0x22 => self.push_str("&quot;"),
                0x3c => self.push_str("&lt;"),
                0x3e => self.push_str("&gt;"),
                0xa0 => self.push_str("&nbsp;"),
                _ => self.0.push(unit),
            }
        }
    }

    fn text(&mut self, units: &[u16], raw: bool) {
        for &unit in units {
            let escaped = if raw {
                None
            } else {
                match unit {
                    0x26 => Some("&amp;"),
                    0x3c => Some("&lt;"),
                    0x3e => Some("&gt;"),
                    0xa0 => Some("&nbsp;"),
                    _ => None,
                }
            };
            if let Some(escaped) = escaped {
                self.push_str(escaped);
            } else {
                self.0.push(unit);
            }
        }
    }
}

#[derive(Default)]
struct Options {
    serializable: bool,
    roots: HashSet<usize>,
}

pub(super) fn fragment_utf16(node: &NodeHandle, state: &HostState) -> Vec<u16> {
    let mut output = HtmlOutput::default();
    // Document.innerHTML is an existing extension: retain its doctype
    // identifiers while Element/ShadowRoot serialization follows HTML rules.
    if node.node_type() == NodeType::Document {
        for child in node.child_nodes() {
            if child.node_type() == NodeType::DocumentType {
                output.0.extend(crate::xml::serialize_utf16(&child));
            } else {
                serialize(&child, false, &Options::default(), state, &mut output);
            }
        }
        return output.0;
    }
    fragment(node, &Options::default(), state, &mut output);
    output.0
}

pub(super) fn get_html_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, id)?;
    let serializable = args.get(1).is_some_and(JsValue::to_boolean);
    let roots: Vec<usize> = serde_json::from_str(
        &args
            .get(2)
            .cloned()
            .unwrap_or_default()
            .to_string(context)?
            .to_std_string_escaped(),
    )
    .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
    for &root in &roots {
        ensure_same_origin_node(context, root)?;
    }
    with_host_state(|state| {
        let state = state.borrow();
        let node = state
            .get_node(id)
            .ok_or_else(|| JsNativeError::reference().with_message("node not found"))?;
        let options = Options {
            serializable,
            roots: roots.into_iter().collect(),
        };
        let mut html = HtmlOutput::default();
        if args.get(3).is_some_and(JsValue::to_boolean) {
            let parent = node.parent_node();
            let raw = parent.as_ref().is_some_and(|parent| {
                is_raw_text_context(parent, scripting_enabled(&state, parent))
            });
            serialize(&node, raw, &options, &state, &mut html);
        } else {
            fragment(&node, &options, &state, &mut html);
        }
        Ok(JsString::from(html.0.as_slice()).into())
    })
}

pub(super) fn settings_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, id)?;
    with_host_state(|state| {
        let node = state
            .borrow()
            .get_node(id)
            .ok_or_else(|| JsNativeError::reference().with_message("root not found"))?;
        let settings = node
            .shadow_root_settings()
            .ok_or_else(|| JsNativeError::typ().with_message("not a shadow root"))?;
        Ok(js_string!(
            serde_json::json!({
                "serializable": settings.serializable,
                "delegatesFocus": settings.delegates_focus,
                "clonable": settings.clonable,
                "slotAssignment": if settings.manual_slot_assignment {"manual"} else {"named"},
            })
            .to_string()
        )
        .into())
    })
}

pub(super) fn set_settings_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, id)?;
    let settings = ShadowRootSettings {
        serializable: args.get(1).is_some_and(JsValue::to_boolean),
        delegates_focus: args.get(2).is_some_and(JsValue::to_boolean),
        clonable: args.get(3).is_some_and(JsValue::to_boolean),
        manual_slot_assignment: args.get(4).is_some_and(JsValue::to_boolean),
        declarative: false,
    };
    with_host_state(|state| {
        let node = state
            .borrow()
            .get_node(id)
            .ok_or_else(|| JsNativeError::reference().with_message("root not found"))?;
        if !node.set_shadow_root_settings(settings) {
            return Err(JsNativeError::typ()
                .with_message("not a shadow root")
                .into());
        }
        Ok(JsValue::undefined())
    })
}

fn html_element(node: &NodeHandle) -> bool {
    node.node_type() == NodeType::Element
        && match node.namespace_uri().as_deref() {
            Some("http://www.w3.org/1999/xhtml") => true,
            // Older native HTML constructors omit namespace metadata. XML
            // constructors also allow no namespace, but retain their branding.
            None => node.is_html_element(),
            _ => false,
        }
}

fn is_void(node: &NodeHandle) -> bool {
    html_element(node)
        && matches!(
            node.local_name().or_else(|| node.tag_name()).as_deref(),
            Some(
                "area"
                    | "base"
                    | "basefont"
                    | "bgsound"
                    | "link"
                    | "meta"
                    | "br"
                    | "col"
                    | "embed"
                    | "hr"
                    | "img"
                    | "input"
                    | "keygen"
                    | "param"
                    | "source"
                    | "track"
                    | "wbr"
            )
        )
}

fn fragment(node: &NodeHandle, options: &Options, state: &HostState, html: &mut HtmlOutput) {
    if is_void(node) {
        return;
    }
    if let Some(root) = node.shadow_root() {
        let settings = root.shadow_root_settings().unwrap_or_default();
        if (options.serializable && settings.serializable)
            || options.roots.contains(&root.identity())
        {
            html.push_str("<template shadowrootmode=\"");
            html.push_str(if root.shadow_root_mode() == Some(ShadowRootMode::Closed) {
                "closed"
            } else {
                "open"
            });
            html.push('"');
            for (enabled, attribute) in [
                (settings.delegates_focus, "shadowrootdelegatesfocus=\"\""),
                (settings.serializable, "shadowrootserializable=\"\""),
                (
                    settings.manual_slot_assignment,
                    "shadowrootslotassignment=\"manual\"",
                ),
                (settings.clonable, "shadowrootclonable=\"\""),
            ] {
                if enabled {
                    html.push(' ');
                    html.push_str(attribute);
                }
            }
            html.push('>');
            fragment(&root, options, state, html);
            html.push_str("</template>");
        }
    }
    let contents = node.template_content().unwrap_or_else(|| node.clone());
    let context = node.shadow_host().unwrap_or_else(|| node.clone());
    let raw = is_raw_text_context(&context, scripting_enabled(state, &context));
    for child in contents.child_nodes() {
        serialize(&child, raw, options, state, html);
    }
}

fn serialize(
    node: &NodeHandle,
    raw: bool,
    options: &Options,
    state: &HostState,
    html: &mut HtmlOutput,
) {
    match node.node_type() {
        NodeType::Element => {
            let tag = serialized_tag_name(node);
            html.push('<');
            html.push_str(&tag);
            if let Some(attributes) = node.attribute_records_utf16() {
                for (name, namespace, local_name, value) in &attributes {
                    html.push(' ');
                    html.push_str(&serialized_attribute_name(
                        name,
                        namespace.as_deref(),
                        local_name,
                    ));
                    html.push_str("=\"");
                    html.attribute(value);
                    html.push('"');
                }
            }
            html.push('>');
            if !is_void(node) {
                fragment(node, options, state, html);
                html.push_str("</");
                html.push_str(&tag);
                html.push('>');
            }
        }
        NodeType::Text => {
            html.text(&node.data_utf16().unwrap_or_default(), raw);
        }
        NodeType::Comment => {
            html.push_str("<!--");
            html.text(&node.data_utf16().unwrap_or_default(), true);
            html.push_str("-->");
        }
        NodeType::ProcessingInstruction => {
            html.push_str("<?");
            html.push_str(&node.node_name());
            html.push(' ');
            html.text(&node.data_utf16().unwrap_or_default(), true);
            html.push_str("?>");
        }
        NodeType::DocumentType => {
            html.push_str("<!DOCTYPE ");
            html.push_str(&node.node_name());
            html.push('>');
        }
        _ => fragment(node, options, state, html),
    }
}

fn serialized_tag_name(node: &NodeHandle) -> String {
    let local = node
        .local_name()
        .or_else(|| node.tag_name())
        .unwrap_or_default();
    match node.namespace_uri().as_deref() {
        Some(
            "http://www.w3.org/1999/xhtml"
            | "http://www.w3.org/2000/svg"
            | "http://www.w3.org/1998/Math/MathML",
        ) => local,
        _ => node
            .prefix()
            .filter(|prefix| !prefix.is_empty())
            .map_or_else(|| local.clone(), |prefix| format!("{prefix}:{local}")),
    }
}

fn serialized_attribute_name(qualified: &str, namespace: Option<&str>, local: &str) -> String {
    match namespace {
        None => local.to_owned(),
        Some("http://www.w3.org/XML/1998/namespace") => format!("xml:{local}"),
        Some("http://www.w3.org/2000/xmlns/") if local == "xmlns" => "xmlns".to_owned(),
        Some("http://www.w3.org/2000/xmlns/") => format!("xmlns:{local}"),
        Some("http://www.w3.org/1999/xlink") => format!("xlink:{local}"),
        _ => qualified.to_owned(),
    }
}

// The host view is borrowed only for this synchronous serialization. Resolve
// each context's owner separately because template contents have an inert owner.
fn scripting_enabled(state: &HostState, node: &NodeHandle) -> bool {
    state
        .node_lifetime_owner(node.identity())
        .is_some_and(|document| {
            state.document_is_active(document.identity())
                && state.sandbox_policy_for_document(&document).allow_scripts
        })
}

fn is_raw_text_context(context: &NodeHandle, scripting_enabled: bool) -> bool {
    html_element(context)
        && match context
            .local_name()
            .or_else(|| context.tag_name())
            .as_deref()
        {
            Some("style" | "script" | "xmp" | "iframe" | "noembed" | "noframes" | "plaintext") => {
                true
            }
            Some("noscript") => scripting_enabled,
            _ => false,
        }
}
