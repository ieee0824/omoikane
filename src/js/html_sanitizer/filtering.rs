use super::*;
use configuration::{Element, contains};

/// Traversal tasks own short-lived handles and never retain document state.
enum Task {
    Visit(NodeHandle, NodeHandle),
    Unwrap(NodeHandle, NodeHandle),
}

pub(super) fn sanitize_children(root: &NodeHandle, config: &Config) {
    let mut tasks = Vec::new();
    enqueue_children(&mut tasks, root);
    while let Some(task) = tasks.pop() {
        match task {
            Task::Unwrap(parent, child) => {
                for inner in child.child_nodes() {
                    parent
                        .insert_before(inner, &child)
                        .expect("parsed child replacement");
                }
                parent.remove_child(&child).expect("parsed child removal");
            }
            Task::Visit(parent, child) => visit_child(&mut tasks, &parent, &child, config),
        }
    }
}

fn enqueue_children(tasks: &mut Vec<Task>, parent: &NodeHandle) {
    for child in parent.child_nodes().into_iter().rev() {
        tasks.push(Task::Visit(parent.clone(), child));
    }
}

fn visit_child(tasks: &mut Vec<Task>, parent: &NodeHandle, child: &NodeHandle, config: &Config) {
    let remove = match child.node_type() {
        NodeType::Comment => config.comments != Some(true),
        NodeType::ProcessingInstruction => {
            let target = configuration::Pi {
                target: child.node_name(),
            };
            config.processing_instructions.as_ref().map_or_else(
                || {
                    config
                        .remove_processing_instructions
                        .as_ref()
                        .is_some_and(|list| list.contains(&target))
                },
                |list| !list.contains(&target),
            )
        }
        NodeType::Element => {
            let key = element_name(child);
            if contains(&config.replace_with_children_elements, &key) {
                tasks.push(Task::Unwrap(parent.clone(), child.clone()));
                enqueue_children(tasks, child);
                return;
            }
            let allowed = config.elements.as_ref().map_or_else(
                || !contains(&config.remove_elements, &key),
                |list| list.iter().any(|entry| entry.key == key),
            );
            if allowed {
                filter_attributes(child, &key, config);
                enqueue_children(tasks, child);
                if let Some(root) = child.shadow_root() {
                    enqueue_children(tasks, &root);
                }
                if let Some(contents) = child.template_content() {
                    enqueue_children(tasks, &contents);
                }
            }
            !allowed
        }
        _ => false,
    };
    if remove {
        parent.remove_child(child).expect("parsed child removal");
    }
}

fn element_name(element: &NodeHandle) -> Name {
    let namespace = if element.is_html_element() {
        Some(HTML.to_owned())
    } else {
        element.namespace_uri()
    };
    Name {
        name: element.local_name().unwrap_or_default(),
        namespace,
    }
}

fn filter_attributes(element: &NodeHandle, key: &Name, config: &Config) {
    let local = config
        .elements
        .as_ref()
        .and_then(|list| list.iter().find(|entry| entry.key == *key));
    for (_, namespace, local_name, value) in element.attribute_records().unwrap_or_default() {
        let attribute = Name::new(local_name, namespace.as_deref());
        if !allowed_attribute(&attribute, local, config)
            || (config.javascript_urls != Some(true)
                && unsafe_url_attribute(key, &attribute, &value))
        {
            element.remove_xml_attribute_ns(attribute.namespace.as_deref(), &attribute.name);
        }
    }
}

fn allowed_attribute(attribute: &Name, local: Option<&Element>, config: &Config) -> bool {
    if local.is_some_and(|entry| contains(&entry.remove_attributes, attribute)) {
        return false;
    }
    if let Some(global) = &config.attributes {
        global.contains(attribute)
            || local.is_some_and(|entry| contains(&entry.attributes, attribute))
            || (config.data_attributes == Some(true) && attribute.is_data())
    } else {
        !contains(&config.remove_attributes, attribute)
            && local
                .and_then(|entry| entry.attributes.as_ref())
                .is_none_or(|list| list.contains(attribute))
    }
}

fn unsafe_url_attribute(element: &Name, attribute: &Name, value: &str) -> bool {
    if element.namespace.as_deref() == Some(SVG)
        && matches!(
            element.name.as_str(),
            "animate" | "animateTransform" | "set"
        )
        && attribute.namespace.is_none()
        && attribute.name == "attributeName"
        && matches!(value, "href" | "xlink:href")
    {
        return true;
    }
    let navigating = CONSTANTS
        .navigating_attributes
        .iter()
        .any(|(node, attr)| node == element && attr == attribute)
        || (element.namespace.as_deref() == Some(MATHML)
            && attribute.name == "href"
            && matches!(attribute.namespace.as_deref(), None | Some(XLINK)));
    navigating && url::Url::parse(value).is_ok_and(|url| url.scheme() == "javascript")
}
