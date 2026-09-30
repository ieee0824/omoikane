//! `Accessibility` domain: snapshots of the accessibility tree and their
//! serialization into CDP AX nodes.

use super::*;

fn accessibility_depth(params: &Value) -> Result<i64, JsonRpcError> {
    match params.get("depth") {
        None => Ok(-1),
        Some(depth) => depth
            .as_u64()
            .and_then(|depth| i64::try_from(depth).ok())
            .ok_or_else(|| {
                invalid_params("Accessibility depth must be a non-negative integer".to_string())
            }),
    }
}

fn collect_ax_nodes<'a>(node: &'a AccessibilityNode, output: &mut Vec<&'a AccessibilityNode>) {
    output.push(node);
    for child in &node.children {
        collect_ax_nodes(child, output);
    }
}

fn collect_reachable_ax_children<'a>(
    node: &'a AccessibilityNode,
    output: &mut Vec<&'a AccessibilityNode>,
) {
    for child in &node.children {
        output.push(child);
        if child.ignored {
            collect_reachable_ax_children(child, output);
        }
    }
}

fn ax_parent_ids(root: &AccessibilityNode) -> HashMap<String, String> {
    fn collect(node: &AccessibilityNode, parent_ids: &mut HashMap<String, String>) {
        for child in &node.children {
            parent_ids.insert(child.node_id.clone(), node.node_id.clone());
            collect(child, parent_ids);
        }
    }

    let mut parent_ids = HashMap::new();
    collect(root, &mut parent_ids);
    parent_ids
}

fn accessibility_tree_target(node: NodeHandle) -> NodeHandle {
    if node.node_type() == NodeType::DocumentFragment {
        node.shadow_host().unwrap_or(node)
    } else {
        node
    }
}

fn ax_composed_parent(node: &NodeHandle) -> Option<NodeHandle> {
    if node.node_type() == NodeType::DocumentFragment {
        return node.shadow_host();
    }
    node.assigned_slot().or_else(|| {
        node.parent_node().and_then(|parent| {
            if parent.node_type() == NodeType::DocumentFragment {
                parent.shadow_host()
            } else {
                Some(parent)
            }
        })
    })
}

fn nearest_ax_ancestor_path<'a>(
    tree: &'a AccessibilityTree,
    target: &NodeHandle,
) -> Option<Vec<&'a AccessibilityNode>> {
    let mut current = ax_composed_parent(target);
    while let Some(ancestor) = current {
        if let Some(path) = tree.path_to_dom_identity(ancestor.identity()) {
            return Some(path);
        }
        current = ax_composed_parent(&ancestor);
    }
    None
}

impl CdpSession {
    fn accessibility_tree(&mut self) -> AccessibilityTree {
        let document = self.runtime.document();
        let title = document
            .query_selector("title")
            .map(|title| dom_text_content(&title))
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| self.current_url.clone());
        let focused_identity = self.runtime.accessibility_focused_node_identity();
        let snapshot_state = self.runtime.accessibility_snapshot_state();
        AccessibilityTree::build(
            &document,
            title,
            self.document_generation,
            focused_identity,
            &snapshot_state,
            |node| self.runtime.accessibility_render_state(node),
        )
    }

    fn accessibility_inspected_node(&mut self, target: &NodeHandle) -> Option<AccessibilityNode> {
        let document = self.runtime.document();
        let focused_identity = self.runtime.accessibility_focused_node_identity();
        let snapshot_state = self.runtime.accessibility_snapshot_state();
        AccessibilityTree::build_inspected_node(
            &document,
            target,
            self.document_generation,
            focused_identity,
            &snapshot_state,
            |node| self.runtime.accessibility_render_state(node),
        )
    }

    pub(super) fn accessibility_get_full_tree(
        &mut self,
        params: &Value,
    ) -> Result<Value, JsonRpcError> {
        self.validate_accessibility_frame(params)?;
        let depth = accessibility_depth(params)?;
        let tree = self.accessibility_tree();
        let mut nodes = Vec::new();
        self.serialize_ax_subtree(&tree.root, None, depth, &mut nodes);
        Ok(json!({ "nodes": nodes }))
    }

    pub(super) fn accessibility_get_root_node(
        &mut self,
        params: &Value,
    ) -> Result<Value, JsonRpcError> {
        self.require_accessibility_enabled()?;
        self.validate_accessibility_frame(params)?;
        let tree = self.accessibility_tree();
        Ok(json!({
            "node": self.serialize_ax_node(&tree.root, None, false),
        }))
    }

    pub(super) fn accessibility_get_partial_tree(
        &mut self,
        params: &Value,
    ) -> Result<Value, JsonRpcError> {
        let target = accessibility_tree_target(self.accessibility_dom_node(params, false)?);
        let tree = self.accessibility_tree();
        let fetch_relatives = params
            .get("fetchRelatives")
            .map(Value::as_bool)
            .unwrap_or(Some(true))
            .ok_or_else(|| invalid_params("fetchRelatives must be a boolean".to_string()))?;
        let Some(path) = tree.path_to_dom_identity(target.identity()) else {
            let reason = self
                .accessibility_inspected_node(&target)
                .and_then(|node| node.ignored_reasons.first().cloned())
                .unwrap_or_else(|| "notRendered".to_string());
            let ancestor_path = fetch_relatives
                .then(|| nearest_ax_ancestor_path(&tree, &target))
                .flatten();
            let parent_id = ancestor_path
                .as_ref()
                .and_then(|path| path.last())
                .map(|parent| parent.node_id.as_str());
            let mut nodes = vec![self.serialize_synthetic_ax_node(&target, &reason, parent_id)];
            if let Some(path) = ancestor_path {
                let parent_ids = ax_parent_ids(&tree.root);
                for (index, ancestor) in path.into_iter().rev().enumerate() {
                    let parent_id = parent_ids.get(&ancestor.node_id);
                    let mut serialized =
                        self.serialize_ax_node(ancestor, parent_id.map(String::as_str), false);
                    if index == 0 {
                        let mut child_ids = serialized["childIds"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default();
                        if !child_ids.iter().any(|id| id == "0") {
                            child_ids.insert(0, Value::String("0".to_string()));
                        }
                        serialized["childIds"] = Value::Array(child_ids);
                    }
                    nodes.push(serialized);
                }
            }
            return Ok(json!({ "nodes": nodes }));
        };
        let target_ax = *path.last().expect("accessibility path contains target");
        let mut ordered = vec![target_ax];
        if fetch_relatives {
            collect_reachable_ax_children(target_ax, &mut ordered);
            if path.len() > 1 {
                for sibling in &path[path.len() - 2].children {
                    if sibling.node_id == target_ax.node_id {
                        continue;
                    }
                    ordered.push(sibling);
                    if sibling.ignored {
                        collect_reachable_ax_children(sibling, &mut ordered);
                    }
                }
            }
            ordered.extend(path[..path.len() - 1].iter().rev().copied());
        }
        let mut seen = HashSet::new();
        ordered.retain(|node| seen.insert(node.node_id.clone()));
        Ok(json!({
            "nodes": self.serialize_ax_nodes_in_order(&tree, ordered, false),
        }))
    }

    pub(super) fn accessibility_get_node_and_ancestors(
        &mut self,
        params: &Value,
    ) -> Result<Value, JsonRpcError> {
        self.require_accessibility_enabled()?;
        let target = accessibility_tree_target(self.accessibility_dom_node(params, false)?);
        let tree = self.accessibility_tree();
        let Some(path) = tree.path_to_dom_identity(target.identity()) else {
            let reason = self
                .accessibility_inspected_node(&target)
                .and_then(|node| node.ignored_reasons.first().cloned())
                .unwrap_or_else(|| "notRendered".to_string());
            let ancestor_path = nearest_ax_ancestor_path(&tree, &target);
            let parent_id = ancestor_path
                .as_ref()
                .and_then(|path| path.last())
                .map(|parent| parent.node_id.as_str());
            let mut nodes = vec![self.serialize_synthetic_ax_node(&target, &reason, parent_id)];
            if let Some(path) = ancestor_path {
                let parent_ids = ax_parent_ids(&tree.root);
                for (index, ancestor) in path.into_iter().rev().enumerate() {
                    let parent_id = parent_ids.get(&ancestor.node_id);
                    let mut serialized =
                        self.serialize_ax_node(ancestor, parent_id.map(String::as_str), false);
                    if index == 0 {
                        let mut child_ids = serialized["childIds"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default();
                        if !child_ids.iter().any(|id| id == "0") {
                            child_ids.insert(0, Value::String("0".to_string()));
                        }
                        serialized["childIds"] = Value::Array(child_ids);
                    }
                    nodes.push(serialized);
                }
            }
            return Ok(json!({
                "nodes": nodes,
            }));
        };
        Ok(json!({
            "nodes": self.serialize_ax_nodes_in_order(
                &tree,
                path.into_iter().rev().collect(),
                false,
            ),
        }))
    }

    pub(super) fn accessibility_get_child_nodes(
        &mut self,
        params: &Value,
    ) -> Result<Value, JsonRpcError> {
        self.require_accessibility_enabled()?;
        self.validate_accessibility_frame(params)?;
        let node_id = require_string(params, "id")?;
        let tree = self.accessibility_tree();
        let node = tree.find_by_node_id(&node_id).ok_or(JsonRpcError {
            code: -32000,
            message: format!("Unknown accessibility node: {node_id}"),
        })?;
        let mut children = Vec::new();
        collect_reachable_ax_children(node, &mut children);
        let nodes = self.serialize_ax_nodes_in_order(&tree, children, false);
        Ok(json!({ "nodes": nodes }))
    }

    pub(super) fn accessibility_query_tree(
        &mut self,
        params: &Value,
    ) -> Result<Value, JsonRpcError> {
        let target = accessibility_tree_target(self.accessibility_dom_node(params, false)?);
        let accessible_name = optional_string(params, "accessibleName")?;
        let role = optional_string(params, "role")?.map(|role| role.to_ascii_lowercase());
        let full_tree = self.accessibility_tree();
        let Some(root) = full_tree.find_by_dom_identity(target.identity()).cloned() else {
            return Ok(json!({ "nodes": [] }));
        };
        let tree = AccessibilityTree { root };
        let mut candidates = Vec::new();
        collect_ax_nodes(&tree.root, &mut candidates);
        let selected = candidates
            .into_iter()
            .filter(|node| {
                accessible_name
                    .as_ref()
                    .is_none_or(|expected| node.name == *expected)
            })
            .filter(|node| {
                role.as_ref()
                    .is_none_or(|expected| node.role.eq_ignore_ascii_case(expected))
            })
            .map(|node| node.node_id.clone())
            .collect::<HashSet<_>>();
        Ok(json!({
            "nodes": self.serialize_query_ax_nodes(&tree, &full_tree, &selected),
        }))
    }

    fn require_accessibility_enabled(&self) -> Result<(), JsonRpcError> {
        if self.accessibility_enabled {
            Ok(())
        } else {
            Err(JsonRpcError {
                code: -32000,
                message: "Accessibility has not been enabled".to_string(),
            })
        }
    }

    fn validate_accessibility_frame(&self, params: &Value) -> Result<(), JsonRpcError> {
        let Some(frame_id) = params.get("frameId") else {
            return Ok(());
        };
        let frame_id = frame_id
            .as_str()
            .ok_or_else(|| invalid_params("frameId must be a string".to_string()))?;
        if frame_id == self.frame_id {
            Ok(())
        } else {
            Err(invalid_params(format!(
                "Frame with the given frameId is not found: {frame_id}"
            )))
        }
    }

    fn accessibility_dom_node(
        &mut self,
        params: &Value,
        allow_document_default: bool,
    ) -> Result<NodeHandle, JsonRpcError> {
        if let Some(object_id) = params.get("objectId") {
            let object_id = object_id.as_str().ok_or_else(|| {
                invalid_params("Accessibility objectId must be a string".to_string())
            })?;
            return self
                .runtime
                .node_for_remote_object_id(object_id)
                .ok_or(JsonRpcError {
                    code: -32000,
                    message: format!("Remote object is not a DOM node: {object_id}"),
                });
        }
        let node_id = params
            .get("nodeId")
            .or_else(|| params.get("backendNodeId"))
            .and_then(Value::as_u64);
        match node_id {
            Some(node_id) => self.lookup_node(node_id),
            None if allow_document_default => Ok(self.runtime.document()),
            None => Err(invalid_params(
                "Missing nodeId or backendNodeId".to_string(),
            )),
        }
    }

    fn serialize_ax_subtree(
        &mut self,
        node: &AccessibilityNode,
        parent_id: Option<&str>,
        depth: i64,
        output: &mut Vec<Value>,
    ) {
        output.push(self.serialize_ax_node(node, parent_id, false));
        if depth == 0 {
            return;
        }
        let next_depth = if depth < 0 { -1 } else { depth - 1 };
        for child in &node.children {
            self.serialize_ax_subtree(child, Some(&node.node_id), next_depth, output);
        }
    }

    fn serialize_ax_nodes_in_order(
        &mut self,
        tree: &AccessibilityTree,
        nodes: Vec<&AccessibilityNode>,
        force_computed_ignored: bool,
    ) -> Vec<Value> {
        let parent_ids = ax_parent_ids(&tree.root);
        nodes
            .into_iter()
            .map(|node| {
                let parent_id = parent_ids.get(&node.node_id).map(String::as_str);
                self.serialize_ax_node(node, parent_id, force_computed_ignored)
            })
            .collect()
    }

    fn serialize_query_ax_nodes(
        &mut self,
        query_tree: &AccessibilityTree,
        full_tree: &AccessibilityTree,
        selected: &HashSet<String>,
    ) -> Vec<Value> {
        let query_parent_ids = ax_parent_ids(&query_tree.root);
        let full_parent_ids = ax_parent_ids(&full_tree.root);
        query_tree
            .nodes_preorder()
            .into_iter()
            .filter(|node| selected.contains(&node.node_id))
            .map(|node| {
                let parent_id = query_parent_ids
                    .get(&node.node_id)
                    .or_else(|| full_parent_ids.get(&node.node_id))
                    .cloned()
                    .or_else(|| {
                        nearest_ax_ancestor_path(full_tree, &node.dom_node)
                            .and_then(|path| path.last().map(|parent| parent.node_id.clone()))
                    });
                self.serialize_ax_node(node, parent_id.as_deref(), true)
            })
            .collect()
    }

    fn serialize_synthetic_ax_node(
        &mut self,
        node: &NodeHandle,
        reason: &str,
        parent_id: Option<&str>,
    ) -> Value {
        let mut payload = json!({
            "nodeId": "0",
            "ignored": true,
            "ignoredReasons": [{
                "name": reason,
                "value": { "type": "boolean", "value": true },
            }],
            "role": { "type": "role", "value": "none" },
            "backendDOMNodeId": self.ensure_node_id(node),
        });
        if let Some(parent_id) = parent_id {
            payload["parentId"] = Value::String(parent_id.to_string());
        }
        payload
    }

    fn serialize_ax_node(
        &mut self,
        node: &AccessibilityNode,
        parent_id: Option<&str>,
        force_computed_ignored: bool,
    ) -> Value {
        let backend_node_id = self.ensure_node_id(&node.dom_node);
        let mut payload = json!({
            "nodeId": node.node_id,
            "ignored": node.ignored,
            "childIds": node.children.iter().map(|child| child.node_id.clone()).collect::<Vec<_>>(),
            "backendDOMNodeId": backend_node_id,
        });
        if let Some(parent_id) = parent_id {
            payload["parentId"] = Value::String(parent_id.to_string());
        } else {
            payload["frameId"] = Value::String(self.frame_id.clone());
        }
        if node.ignored && !force_computed_ignored {
            payload["role"] = json!({ "type": "role", "value": "none" });
        }
        if !node.ignored {
            payload["role"] = json!({ "type": "role", "value": node.role });
            payload["name"] = json!({ "type": "computedString", "value": node.name });
            payload["properties"] = Value::Array(
                node.properties
                    .iter()
                    .map(|property| self.serialize_ax_property(property))
                    .collect(),
            );
            if !node.description.is_empty() {
                payload["description"] = json!({
                    "type": "computedString",
                    "value": node.description,
                });
            }
            if let Some(value) = node.value.as_ref() {
                payload["value"] = self.serialize_ax_value(value, "string");
            }
        } else if force_computed_ignored {
            payload["role"] = json!({ "type": "role", "value": node.role });
            payload["name"] = json!({ "type": "computedString", "value": node.name });
        }
        if node.ignored {
            payload["ignoredReasons"] = Value::Array(
                node.ignored_reasons
                    .iter()
                    .map(|reason| {
                        json!({
                            "name": reason,
                            "value": { "type": "boolean", "value": true },
                        })
                    })
                    .collect(),
            );
        }
        payload
    }

    fn serialize_ax_property(&mut self, property: &AccessibilityProperty) -> Value {
        json!({
            "name": property.name,
            "value": self.serialize_ax_value(&property.value, "string"),
        })
    }

    fn serialize_ax_value(&mut self, value: &AccessibilityValue, string_type: &str) -> Value {
        match value {
            AccessibilityValue::Boolean(value) => {
                json!({ "type": "boolean", "value": value })
            }
            AccessibilityValue::Integer(value) => {
                json!({ "type": "integer", "value": value })
            }
            AccessibilityValue::Number(value) => json!({ "type": "number", "value": value }),
            AccessibilityValue::String(value) => {
                json!({ "type": string_type, "value": value })
            }
            AccessibilityValue::Token(value) => json!({ "type": "token", "value": value }),
            AccessibilityValue::TokenList(value) => {
                json!({ "type": "tokenList", "value": value })
            }
            AccessibilityValue::Tristate(value) => {
                json!({ "type": "tristate", "value": value })
            }
            AccessibilityValue::IdRef {
                value,
                related_nodes,
            } => json!({
                "type": "idref",
                "value": value,
                "relatedNodes": related_nodes.iter().map(|related| {
                    json!({
                        "backendDOMNodeId": self.ensure_node_id(&related.dom_node),
                        "idref": related.idref,
                        "text": related.text,
                    })
                }).collect::<Vec<_>>(),
            }),
            AccessibilityValue::IdRefList {
                value,
                related_nodes,
            } => json!({
                "type": "idrefList",
                "value": value,
                "relatedNodes": related_nodes.iter().map(|related| {
                    json!({
                        "backendDOMNodeId": self.ensure_node_id(&related.dom_node),
                        "idref": related.idref,
                        "text": related.text,
                    })
                }).collect::<Vec<_>>(),
            }),
        }
    }
}
