//! Collect validation topology without allocating JS wrappers for unrelated DOM nodes.
use super::*;

#[derive(serde::Serialize)]
struct Record {
    id: usize,
    tag: String,
    parent: i64,
    form: Option<usize>,
}

#[derive(serde::Serialize)]
struct Tree {
    records: Vec<Record>,
    first_ids: Vec<usize>,
}

fn collect_tree(root: NodeHandle, shadows: &mut Vec<NodeHandle>) -> Tree {
    let mut tree = Tree {
        records: Vec::new(),
        first_ids: Vec::new(),
    };
    let mut seen_ids = HashSet::new();
    let mut pending = vec![(root, -1, None)];
    while let Some((node, mut parent, mut form)) = pending.pop() {
        if node.node_type() == NodeType::Element {
            if let Some(id) = node.get_attribute_utf16("id").filter(|id| !id.is_empty()) {
                if seen_ids.insert(id) {
                    tree.first_ids.push(node.identity());
                }
            }
            if let Some(shadow) = node.shadow_root() {
                shadows.push(shadow);
            }
            let html = node.is_html_element()
                || node.namespace_uri().as_deref() == Some("http://www.w3.org/1999/xhtml");
            let tag = html
                .then(|| node.local_name().unwrap_or_default())
                .unwrap_or_default();
            let is_form = tag == "form";
            if matches!(
                tag.as_str(),
                "input" | "select" | "textarea" | "button" | "form" | "fieldset"
            ) || node.is_form_associated_custom()
                || node.css_validity().is_some()
            {
                let index = tree.records.len() as i64;
                tree.records.push(Record {
                    id: node.identity(),
                    tag,
                    parent,
                    form,
                });
                parent = index;
            }
            if is_form {
                form = Some(node.identity());
            }
        }
        for child in node.child_nodes().into_iter().rev() {
            pending.push((child, parent, form));
        }
    }
    tree
}

fn snapshot(root: NodeHandle) -> Vec<Tree> {
    let mut roots = vec![root];
    let mut trees = Vec::new();
    while let Some(root) = roots.pop() {
        trees.push(collect_tree(root, &mut roots));
    }
    trees
}

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_validation_tree"),
        1,
        NativeFunction::from_copy_closure(|_, args, context| {
            let id = parse_node_id(args.first(), context)?;
            ensure_same_origin_node(context, id)?;
            with_host_state(|host| {
                let state = host.borrow();
                let root = state.get_node(id).ok_or_else(|| {
                    JsNativeError::error().with_message("validation root not found")
                })?;
                let json = serde_json::to_string(&snapshot(root))
                    .map_err(|error| JsNativeError::error().with_message(error.to_string()))?;
                Ok(js_string!(json.as_str()).into())
            })
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unrelated_nodes_do_not_expand_validation_records_or_break_fieldset_ancestry() {
        let document = crate::html::TreeBuilder::parse(&format!(
            "<form><fieldset><section>{}<input required></section></fieldset></form>",
            "<div><span>unrelated text</span></div>".repeat(1000)
        ))
        .document();
        let trees = snapshot(document);
        assert_eq!(trees.len(), 1);
        let records = &trees[0].records;
        assert_eq!(records.len(), 3);
        assert_eq!(
            records.iter().map(|r| r.tag.as_str()).collect::<Vec<_>>(),
            ["form", "fieldset", "input"]
        );
        assert_eq!(records[0].parent, -1);
        assert_eq!(records[1].parent, 0);
        assert_eq!(records[2].parent, 1);
        assert_eq!(records[2].form, Some(records[0].id));
    }

    #[test]
    fn first_id_can_belong_to_a_non_form_and_closed_shadow_trees_are_separate() {
        let root = NodeHandle::element("div");
        let blocker = NodeHandle::element("span");
        blocker.set_attribute("id", "owner");
        root.append_child(blocker.clone());
        let form = NodeHandle::element("form");
        form.set_attribute("id", "owner");
        root.append_child(form.clone());
        let shadow = form
            .attach_shadow(crate::dom::ShadowRootMode::Closed)
            .unwrap();
        let input = NodeHandle::element("input");
        shadow.append_child(input.clone());
        let trees = snapshot(root);
        assert_eq!(trees.len(), 2);
        assert_eq!(trees[0].first_ids, [blocker.identity()]);
        assert_eq!(trees[1].records.len(), 1);
        assert_eq!(trees[1].records[0].id, input.identity());
        assert_eq!(trees[1].records[0].form, None);
        assert_eq!(trees[1].records[0].parent, -1);
    }

    #[test]
    fn first_id_index_distinguishes_lone_utf16_surrogates() {
        let root = NodeHandle::element("div");
        let first = NodeHandle::element("form");
        let second = NodeHandle::element("form");
        first.set_attribute_utf16("id", &[0xd800]);
        second.set_attribute_utf16("id", &[0xd801]);
        root.append_child(first.clone());
        root.append_child(second.clone());
        let trees = snapshot(root);
        assert_eq!(trees[0].first_ids, [first.identity(), second.identity()]);
    }

    #[test]
    fn custom_controls_and_stale_validity_survive_candidate_filtering() {
        let root = NodeHandle::element("div");
        let custom = NodeHandle::element("custom-control");
        custom.set_form_associated_custom(true);
        let stale = NodeHandle::element("retired-control");
        stale.set_css_validity(Some(false));
        root.append_child(custom.clone());
        root.append_child(stale.clone());
        let trees = snapshot(root);
        assert_eq!(
            trees[0].records.iter().map(|r| r.id).collect::<Vec<_>>(),
            [custom.identity(), stale.identity()]
        );
    }

    #[test]
    fn prefixed_html_and_xhtml_controls_keep_local_names_without_accepting_svg_controls() {
        let root = NodeHandle::element("div");
        let form = NodeHandle::html_element_ns("h:form", "http://www.w3.org/1999/xhtml");
        let input = NodeHandle::xml_element("h:input", Some("http://www.w3.org/1999/xhtml".into()));
        form.append_child(input.clone());
        root.append_child(form.clone());
        root.append_child(NodeHandle::xml_element(
            "input",
            Some("http://www.w3.org/2000/svg".into()),
        ));
        let trees = snapshot(root);
        assert_eq!(trees[0].records.len(), 2);
        assert_eq!(trees[0].records[0].tag, "form");
        assert_eq!(trees[0].records[1].tag, "input");
        assert_eq!(trees[0].records[1].form, Some(form.identity()));
    }

    #[test]
    fn compressed_validation_preserves_fieldsets_external_owners_and_radio_groups() {
        let document = crate::html::TreeBuilder::parse(
            "<span id=blocked></span><form id=blocked></form>\
             <form id=owner><fieldset><div><input id=a name=r type=radio required>\
             <input id=b name=r type=radio></div></fieldset></form>\
             <input id=external form=owner required><input id=orphan form=blocked required>",
        )
        .document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        runtime.eval(r#"
            const a = document.getElementById('a');
            const b = document.getElementById('b');
            const owner = document.getElementById('owner');
            const fieldset = document.querySelector('fieldset');
            if (!a.matches(':invalid') || !b.matches(':invalid') ||
                !fieldset.matches(':invalid') || !owner.matches(':invalid')) throw Error('initial state');
            b.checked = true;
            if (!a.matches(':valid') || !b.matches(':valid') ||
                !fieldset.matches(':valid') || !owner.matches(':invalid')) throw Error('external control');
            document.getElementById('external').value = 'filled';
            if (!owner.matches(':valid') || document.querySelector('form').matches(':invalid')) throw Error('owner mapping');
            if (typeof globalThis.__omoikane_validation_tree !== 'undefined') throw Error('private binding leaked');
        "#).unwrap();
    }
}
