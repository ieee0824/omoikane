//! `DOM` domain: node ids, document and query commands, and node
//! serialization.

use super::*;

pub(super) fn dom_text_content(node: &NodeHandle) -> String {
    if node.node_type() == NodeType::Text {
        return node.data().unwrap_or_default();
    }
    node.child_nodes()
        .iter()
        .map(dom_text_content)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn cdp_node_type(node: &NodeHandle) -> u8 {
    if node.is_cdata_section() {
        return 4;
    }
    match node.node_type() {
        NodeType::Element => 1,
        NodeType::Text => 3,
        NodeType::ProcessingInstruction => 7,
        NodeType::Comment => 8,
        NodeType::Document => 9,
        NodeType::DocumentType => 10,
        NodeType::DocumentFragment => 11,
    }
}

pub(super) fn serialize_outer_html(node: &NodeHandle) -> String {
    match node.node_type() {
        NodeType::Document => node
            .child_nodes()
            .iter()
            .map(serialize_outer_html)
            .collect::<Vec<_>>()
            .join(""),
        NodeType::Element => {
            let tag_name = node.tag_name().unwrap_or_default();
            let attributes = node
                .attributes()
                .unwrap_or_default()
                .into_iter()
                .map(|(name, value)| format!(r#" {name}="{}""#, escape_html(&value)))
                .collect::<Vec<_>>()
                .join("");
            let children = node
                .child_nodes()
                .iter()
                .map(serialize_outer_html)
                .collect::<Vec<_>>()
                .join("");
            format!("<{tag_name}{attributes}>{children}</{tag_name}>")
        }
        NodeType::Text if node.is_cdata_section() => {
            format!("<![CDATA[{}]]>", node.data().unwrap_or_default())
        }
        NodeType::Text => escape_html(&node.data().unwrap_or_default()),
        NodeType::Comment => format!("<!--{}-->", node.data().unwrap_or_default()),
        NodeType::ProcessingInstruction => {
            let data = node.data().unwrap_or_default();
            if data.is_empty() {
                format!("<?{}?>", node.node_name())
            } else {
                format!("<?{} {}?>", node.node_name(), data)
            }
        }
        NodeType::DocumentType => format!("<!DOCTYPE {}>", node.data().unwrap_or_default()),
        NodeType::DocumentFragment => node
            .child_nodes()
            .iter()
            .map(serialize_outer_html)
            .collect::<Vec<_>>()
            .join(""),
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

impl CdpSession {
    pub(super) fn dom_get_document(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let depth = params.get("depth").and_then(Value::as_i64).unwrap_or(-1);
        let document = self.runtime.document();
        Ok(json!({
            "root": self.serialize_node(&document, depth),
        }))
    }

    pub(super) fn dom_query_selector(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let node_id = require_u64(params, "nodeId")?;
        let selector = require_string(params, "selector")?;
        let node = self.lookup_node(node_id)?;
        let result = node
            .query_selector(&selector)
            .map(|node| self.ensure_node_id(&node))
            .unwrap_or(0);
        Ok(json!({ "nodeId": result }))
    }

    pub(super) fn dom_get_attributes(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let node_id = require_u64(params, "nodeId")?;
        let node = self.lookup_node(node_id)?;
        let attributes = node
            .attributes()
            .unwrap_or_default()
            .into_iter()
            .flat_map(|(name, value)| [Value::String(name), Value::String(value)])
            .collect::<Vec<_>>();
        Ok(json!({ "attributes": attributes }))
    }

    pub(super) fn dom_get_outer_html(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let node_id = require_u64(params, "nodeId")?;
        let node = self.lookup_node(node_id)?;
        Ok(json!({
            "outerHTML": serialize_outer_html(&node),
        }))
    }

    pub(super) fn rebuild_node_index(&mut self) {
        self.node_to_id.clear();
        self.id_to_node.clear();
        // Node ids are session-scoped remote handles. Never reuse an id after
        // navigation: a client retaining an old Document's id must receive an
        // unknown-node error, not accidentally address a similarly positioned
        // node in the newly installed Document.
        let document = self.runtime.document();
        self.register_subtree(&document);
    }

    fn register_subtree(&mut self, node: &NodeHandle) {
        self.ensure_node_id(node);
        for child in node.child_nodes() {
            self.register_subtree(&child);
        }
    }

    pub(super) fn ensure_node_id(&mut self, node: &NodeHandle) -> u64 {
        let identity = node.identity();
        if let Some(node_id) = self.node_to_id.get(&identity) {
            return *node_id;
        }

        let node_id = self.next_node_id;
        self.next_node_id += 1;
        self.node_to_id.insert(identity, node_id);
        self.id_to_node.insert(node_id, node.clone());
        node_id
    }

    pub(super) fn lookup_node(&self, node_id: u64) -> Result<NodeHandle, JsonRpcError> {
        self.id_to_node.get(&node_id).cloned().ok_or(JsonRpcError {
            code: -32000,
            message: format!("Unknown node: {node_id}"),
        })
    }

    fn serialize_node(&mut self, node: &NodeHandle, depth: i64) -> Value {
        let node_id = self.ensure_node_id(node);
        let children = if depth == 0 {
            None
        } else {
            let next_depth = if depth < 0 { -1 } else { depth - 1 };
            Some(
                node.child_nodes()
                    .iter()
                    .map(|child| self.serialize_node(child, next_depth))
                    .collect::<Vec<_>>(),
            )
        };

        let local_name = match node.node_type() {
            NodeType::Element => node.tag_name().unwrap_or_default(),
            _ => String::new(),
        };
        let node_value = match node.node_type() {
            NodeType::Text | NodeType::Comment | NodeType::DocumentType => {
                node.data().unwrap_or_default()
            }
            _ => String::new(),
        };

        let mut payload = json!({
            "nodeId": node_id,
            "nodeType": cdp_node_type(node),
            "nodeName": node.node_name(),
            "localName": local_name,
            "nodeValue": node_value,
            "childNodeCount": node.child_nodes().len(),
        });

        if let Some(attributes) = node.attributes() {
            let flattened = attributes
                .into_iter()
                .flat_map(|(name, value)| [Value::String(name), Value::String(value)])
                .collect::<Vec<_>>();
            payload["attributes"] = Value::Array(flattened);
        }

        if let Some(children) = children {
            payload["children"] = Value::Array(children);
        }

        payload
    }
}
