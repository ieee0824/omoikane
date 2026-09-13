use std::collections::HashMap;

use crate::dom::{Node, NodeHandle, NodeType};

const HTML_NAMESPACE: &str = "http://www.w3.org/1999/xhtml";
const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NAMESPACE: &str = "http://www.w3.org/2000/xmlns/";

type Namespace = Option<String>;

#[derive(Clone, Debug)]
struct AttributeRecord {
    qualified_name: String,
    namespace: Namespace,
    local_name: String,
    value: String,
}

impl AttributeRecord {
    fn prefix(&self) -> Option<&str> {
        self.qualified_name
            .split_once(':')
            .map(|(prefix, _)| prefix)
    }
}

#[derive(Debug, Default)]
struct NamespacePrefixMap {
    prefixes: HashMap<Namespace, Vec<String>>,
    history: Vec<Namespace>,
    non_xml_prefixes: usize,
}

impl NamespacePrefixMap {
    fn add(&mut self, namespace: Namespace, prefix: impl Into<String>) {
        let prefix = prefix.into();
        if namespace.as_deref() != Some(XML_NAMESPACE) || prefix != "xml" {
            self.non_xml_prefixes += 1;
        }
        self.prefixes
            .entry(namespace.clone())
            .or_default()
            .push(prefix);
        self.history.push(namespace);
    }

    fn checkpoint(&self) -> usize {
        self.history.len()
    }

    fn restore(&mut self, checkpoint: usize) {
        while self.history.len() > checkpoint {
            let namespace = self.history.pop().expect("namespace history entry");
            let remove_namespace = {
                let prefixes = self
                    .prefixes
                    .get_mut(&namespace)
                    .expect("namespace prefix entry");
                let prefix = prefixes.pop().expect("namespace prefix");
                if namespace.as_deref() != Some(XML_NAMESPACE) || prefix != "xml" {
                    self.non_xml_prefixes -= 1;
                }
                prefixes.is_empty()
            };
            if remove_namespace {
                self.prefixes.remove(&namespace);
            }
        }
    }

    fn contains(&self, namespace: &Namespace, prefix: &str) -> bool {
        self.prefixes
            .get(namespace)
            .is_some_and(|prefixes| prefixes.iter().any(|candidate| candidate == prefix))
    }

    fn preferred(&self, namespace: &Namespace, preferred: Option<&str>) -> Option<String> {
        let candidates = self.prefixes.get(namespace)?;
        if let Some(preferred) = preferred
            && candidates.iter().any(|candidate| candidate == preferred)
        {
            return Some(preferred.to_string());
        }
        candidates.last().cloned()
    }

    fn has_non_xml_prefix(&self) -> bool {
        self.non_xml_prefixes != 0
    }
}

/// Serializes a DOM node using the XML serialization algorithm.
///
/// The Web-facing `XMLSerializer` invokes this with `require well-formed` set
/// to false, so every node representable by Omoikane's native DOM has a string
/// result. Namespace declarations may be rewritten to preserve the namespace
/// identity of elements and attributes when the result is parsed again.
pub fn serialize(node: &NodeHandle) -> String {
    let mut namespaces = NamespacePrefixMap::default();
    namespaces.add(Some(XML_NAMESPACE.to_string()), "xml");
    let mut prefix_index = 1;
    let mut output = String::new();
    serialize_node(node, None, &mut namespaces, &mut prefix_index, &mut output);
    output
}

fn serialize_node(
    node: &NodeHandle,
    inherited_namespace: Namespace,
    namespaces: &mut NamespacePrefixMap,
    prefix_index: &mut usize,
    output: &mut String,
) {
    if node.is_cdata_section() {
        output.push_str("<![CDATA[");
        output.push_str(&node.data().unwrap_or_default());
        output.push_str("]]>");
        return;
    }

    match node.node_type() {
        NodeType::Element => {
            serialize_element(node, inherited_namespace, namespaces, prefix_index, output)
        }
        NodeType::Document | NodeType::DocumentFragment => {
            for child in node.child_nodes() {
                serialize_node(
                    &child,
                    inherited_namespace.clone(),
                    namespaces,
                    prefix_index,
                    output,
                );
            }
        }
        NodeType::Text => append_escaped_text(&node.data().unwrap_or_default(), output),
        NodeType::Comment => {
            output.push_str("<!--");
            output.push_str(&node.data().unwrap_or_default());
            output.push_str("-->");
        }
        NodeType::ProcessingInstruction => {
            output.push_str("<?");
            output.push_str(&node.node_name());
            output.push(' ');
            output.push_str(&node.data().unwrap_or_default());
            output.push_str("?>");
        }
        NodeType::DocumentType => serialize_document_type(node, output),
    }
}

fn serialize_element(
    node: &NodeHandle,
    inherited_namespace: Namespace,
    namespaces: &mut NamespacePrefixMap,
    prefix_index: &mut usize,
    output: &mut String,
) {
    let attributes = node
        .attribute_records()
        .unwrap_or_default()
        .into_iter()
        .map(
            |(qualified_name, namespace, local_name, value)| AttributeRecord {
                qualified_name,
                namespace,
                local_name,
                value,
            },
        )
        .collect::<Vec<_>>();
    let namespace_checkpoint = namespaces.checkpoint();
    let mut local_prefixes = HashMap::new();
    let (local_default_namespace, has_empty_namespace_prefix) =
        record_namespace_information(&attributes, namespaces, &mut local_prefixes);

    let namespace = effective_element_namespace(node);
    let local_name = node.local_name().unwrap_or_else(|| node.node_name());
    let original_prefix = node.prefix();
    let mut child_namespace = inherited_namespace.clone();
    let mut ignore_default_namespace_attribute = false;
    let mut generated_declaration = String::new();

    let qualified_name = if inherited_namespace == namespace {
        if local_default_namespace.is_some() && !has_empty_namespace_prefix {
            ignore_default_namespace_attribute = true;
        }
        if namespace.as_deref() == Some(XML_NAMESPACE) {
            format!("xml:{local_name}")
        } else {
            local_name.clone()
        }
    } else {
        let mut candidate = namespaces.preferred(&namespace, original_prefix.as_deref());
        if original_prefix.as_deref() == Some("xmlns") {
            candidate = Some("xmlns".to_string());
        }

        if let Some(candidate) = candidate {
            if let Some(local_default) = &local_default_namespace
                && local_default != XML_NAMESPACE
            {
                child_namespace = normalize_namespace(local_default);
            }
            format!("{candidate}:{local_name}")
        } else if let Some(mut prefix) = original_prefix {
            if local_prefixes.contains_key(&prefix) {
                prefix = generate_prefix(namespaces, namespace.clone(), prefix_index);
            } else {
                namespaces.add(namespace.clone(), prefix.clone());
            }
            generated_declaration.push_str(" xmlns:");
            generated_declaration.push_str(&prefix);
            generated_declaration.push_str("=\"");
            append_escaped_attribute(
                namespace.as_deref().unwrap_or(""),
                &mut generated_declaration,
            );
            generated_declaration.push('"');
            if let Some(local_default) = &local_default_namespace {
                child_namespace = normalize_namespace(local_default);
            }
            format!("{prefix}:{local_name}")
        } else if local_default_namespace
            .as_ref()
            .map(|value| normalize_namespace(value))
            .as_ref()
            != Some(&namespace)
        {
            ignore_default_namespace_attribute = true;
            child_namespace = namespace.clone();
            generated_declaration.push_str(" xmlns=\"");
            append_escaped_attribute(
                namespace.as_deref().unwrap_or(""),
                &mut generated_declaration,
            );
            generated_declaration.push('"');
            local_name.clone()
        } else {
            child_namespace = namespace.clone();
            local_name.clone()
        }
    };

    output.push('<');
    output.push_str(&qualified_name);
    output.push_str(&generated_declaration);
    serialize_attributes(
        &attributes,
        output,
        namespaces,
        prefix_index,
        &mut local_prefixes,
        ignore_default_namespace_attribute,
    );

    let children = if namespace.as_deref() == Some(HTML_NAMESPACE)
        && local_name.eq_ignore_ascii_case("template")
    {
        node.template_content()
            .map(|content| content.child_nodes())
            .unwrap_or_default()
    } else {
        node.child_nodes()
    };
    let html_namespace = namespace.as_deref() == Some(HTML_NAMESPACE);
    let html_void = html_namespace && is_html_void_element(&local_name);
    if children.is_empty() && !html_namespace {
        output.push_str("/>");
        namespaces.restore(namespace_checkpoint);
        return;
    }
    if children.is_empty() && html_void {
        output.push_str(" />");
        namespaces.restore(namespace_checkpoint);
        return;
    }

    output.push('>');
    for child in children {
        serialize_node(
            &child,
            child_namespace.clone(),
            namespaces,
            prefix_index,
            output,
        );
    }
    output.push_str("</");
    output.push_str(&qualified_name);
    output.push('>');
    namespaces.restore(namespace_checkpoint);
}

fn record_namespace_information(
    attributes: &[AttributeRecord],
    map: &mut NamespacePrefixMap,
    local_prefixes: &mut HashMap<String, String>,
) -> (Option<String>, bool) {
    let mut default_namespace = None;
    let mut has_empty_namespace_prefix = false;
    for attribute in attributes {
        if attribute.namespace.as_deref() != Some(XMLNS_NAMESPACE) {
            continue;
        }
        let Some(prefix) = attribute.prefix() else {
            default_namespace = Some(attribute.value.clone());
            continue;
        };
        if prefix != "xmlns" || attribute.value == XML_NAMESPACE {
            continue;
        }
        let namespace = normalize_namespace(&attribute.value);
        has_empty_namespace_prefix |= namespace.is_none();
        if map.contains(&namespace, &attribute.local_name) {
            continue;
        }
        map.add(namespace, attribute.local_name.clone());
        local_prefixes.insert(attribute.local_name.clone(), attribute.value.clone());
    }
    (default_namespace, has_empty_namespace_prefix)
}

fn serialize_attributes(
    attributes: &[AttributeRecord],
    output: &mut String,
    map: &mut NamespacePrefixMap,
    prefix_index: &mut usize,
    local_prefixes: &mut HashMap<String, String>,
    ignore_default_namespace_attribute: bool,
) {
    for attribute in attributes {
        let namespace = &attribute.namespace;
        let prefix = attribute.prefix();
        let mut candidate = None;

        if namespace.is_some() {
            candidate = map.preferred(namespace, prefix);
            if namespace.as_deref() == Some(XMLNS_NAMESPACE) {
                let inherited_duplicate = prefix.is_some()
                    && local_prefixes
                        .get(&attribute.local_name)
                        .is_none_or(|value| value != &attribute.value)
                    && map.contains(
                        &normalize_namespace(&attribute.value),
                        &attribute.local_name,
                    );
                if attribute.value == XML_NAMESPACE
                    || (prefix.is_none() && ignore_default_namespace_attribute)
                    || inherited_duplicate
                {
                    continue;
                }
                if prefix == Some("xmlns") {
                    candidate = Some("xmlns".to_string());
                }
            } else if candidate.is_none() {
                candidate = if let Some(prefix) = prefix
                    && !map.has_non_xml_prefix()
                {
                    map.add(namespace.clone(), prefix.to_string());
                    Some(prefix.to_string())
                } else {
                    Some(generate_prefix(map, namespace.clone(), prefix_index))
                };
                let candidate = candidate.as_ref().expect("generated attribute prefix");
                local_prefixes.insert(candidate.clone(), namespace.clone().unwrap_or_default());
                output.push_str(" xmlns:");
                output.push_str(candidate);
                output.push_str("=\"");
                append_escaped_attribute(namespace.as_deref().unwrap_or(""), output);
                output.push('"');
            }
        } else if attribute.local_name == "xmlns" {
            // A namespace-less attribute named `xmlns` cannot round-trip as a
            // namespace-less attribute through an XML parser.
            continue;
        }

        output.push(' ');
        if let Some(candidate) = candidate {
            output.push_str(&candidate);
            output.push(':');
        }
        output.push_str(&attribute.local_name);
        output.push_str("=\"");
        append_escaped_attribute(&attribute.value, output);
        output.push('"');
    }
}

fn effective_element_namespace(node: &NodeHandle) -> Namespace {
    node.namespace_uri()
        .or_else(|| node.is_html_element().then(|| HTML_NAMESPACE.to_string()))
}

fn normalize_namespace(value: &str) -> Namespace {
    (!value.is_empty()).then(|| value.to_string())
}

fn generate_prefix(
    map: &mut NamespacePrefixMap,
    namespace: Namespace,
    prefix_index: &mut usize,
) -> String {
    let prefix = format!("ns{prefix_index}");
    *prefix_index += 1;
    map.add(namespace, prefix.clone());
    prefix
}

fn append_escaped_text(value: &str, output: &mut String) {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            _ => output.push(character),
        }
    }
}

fn append_escaped_attribute(value: &str, output: &mut String) {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '"' => output.push_str("&quot;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '\t' => output.push_str("&#9;"),
            '\n' => output.push_str("&#xA;"),
            '\r' => output.push_str("&#xD;"),
            _ => output.push(character),
        }
    }
}

fn serialize_document_type(node: &NodeHandle, output: &mut String) {
    output.push_str("<!DOCTYPE ");
    output.push_str(&node.node_name());
    if let Some(public_id) = node.public_id() {
        output.push_str(" PUBLIC ");
        output.push_str(&quote_identifier(&public_id));
        if let Some(system_id) = node.system_id() {
            output.push(' ');
            output.push_str(&quote_identifier(&system_id));
        }
    } else if let Some(system_id) = node.system_id() {
        output.push_str(" SYSTEM ");
        output.push_str(&quote_identifier(&system_id));
    }
    output.push('>');
}

fn quote_identifier(identifier: &str) -> String {
    let quote = if identifier.contains('"') { '\'' } else { '"' };
    format!("{quote}{identifier}{quote}")
}

fn is_html_void_element(local_name: &str) -> bool {
    const HTML_VOID_ELEMENTS: &[&str] = &[
        "area", "base", "basefont", "bgsound", "br", "col", "embed", "frame", "hr", "img", "input",
        "keygen", "link", "menuitem", "meta", "param", "source", "track", "wbr",
    ];
    HTML_VOID_ELEMENTS
        .iter()
        .any(|element| local_name.eq_ignore_ascii_case(element))
}
