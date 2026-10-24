//! Bounded provenance for large destructive DOM operations under paint tracing.

use std::cell::Cell;
use std::sync::OnceLock;

use crate::dom::{Node, NodeHandle};
use boa_engine::Context;

const SCAN_LIMIT: usize = 512;
const LARGE_REMOVAL: usize = 128;
const RECORD_LIMIT: usize = 64;
const FRAME_LIMIT: usize = 12;

thread_local! {
    static RECORDS: Cell<usize> = const { Cell::new(0) };
}

/// Inspects only owned DOM metadata and immutable VM frames, without running JS.
/// Called immediately before an operation that removes the specified subtree.
pub(super) fn trace(
    api: &'static str,
    target: &NodeHandle,
    children_only: bool,
    context: &Context,
) {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if !*ENABLED.get_or_init(|| std::env::var_os("OMOIKANE_TRACE_PAINT").is_some())
        || RECORDS.with(|records| records.get() >= RECORD_LIMIT)
    {
        return;
    }
    let Some(mut data) = removal(api, target, children_only) else {
        return;
    };
    data["frames"] = frames(context);
    RECORDS.with(|records| records.set(records.get() + 1));
    eprintln!(
        "{}",
        crate::diagnostics::json_record("OMOIKANE_DOM_REMOVAL", &data)
    );
}

fn frames(context: &Context) -> serde_json::Value {
    context
        .stack_trace()
        .take(FRAME_LIMIT)
        .map(|frame| {
            let location = frame.position();
            let source = match &location.path {
                boa_engine::vm::SourcePath::Path(path) => source_label(&path.to_string_lossy()),
                _ => "<anonymous>".into(),
            };
            serde_json::json!({
                "source": source,
                "line": location.position.map(|position| position.line_number()),
                "column": location.position.map(|position| position.column_number()),
            })
        })
        .collect::<Vec<_>>()
        .into()
}

fn removal(api: &str, target: &NodeHandle, children_only: bool) -> Option<serde_json::Value> {
    let mut document = target.clone();
    while let Some(parent) = document.parent_node() {
        document = parent;
    }
    if document.node_type() != crate::dom::NodeType::Document {
        return None;
    }
    let mut current = if children_only {
        target.first_child()
    } else {
        Some(target.clone())
    };
    let mut count = 0;
    while let Some(node) = current.take() {
        if count == SCAN_LIMIT {
            current = Some(node);
            break;
        }
        count += 1;
        current = node.first_child().or_else(|| next(&node, target));
    }
    if count < LARGE_REMOVAL {
        return None;
    }
    Some(serde_json::json!({
        "api": api, "document_id": document.identity(), "target_id": target.identity(),
        "target_tag": target.tag_name(), "children_only": children_only,
        "removed_nodes": count, "removed_truncated": current.is_some(),
        "stage": "before_mutation",
    }))
}

fn next(node: &NodeHandle, root: &NodeHandle) -> Option<NodeHandle> {
    let mut node = node.clone();
    while node != *root {
        if let Some(sibling) = node.next_sibling() {
            return Some(sibling);
        }
        node = node.parent_node()?;
    }
    None
}

/// Removes credentials, query strings, fragments and local paths from a diagnostic source.
pub(super) fn source_label(source: &str) -> String {
    match url::Url::parse(source) {
        Ok(mut url) if matches!(url.scheme(), "http" | "https") => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.set_query(None);
            url.set_fragment(None);
            url.to_string()
        }
        _ => "<anonymous-or-local>".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tree(count: usize) -> (NodeHandle, NodeHandle) {
        let document = crate::html::TreeBuilder::parse("<body></body>").document();
        let body = document.document_body().unwrap();
        for _ in 0..count {
            body.append_child(NodeHandle::element("div"));
        }
        (document, body)
    }

    #[test]
    fn counts_only_removed_nodes_and_never_leaves_the_target_subtree() {
        let (document, body) = tree(LARGE_REMOVAL);
        let data = removal("textContent", &body, true).unwrap();
        assert_eq!(data["document_id"], document.identity());
        assert_eq!(data["removed_nodes"], LARGE_REMOVAL);
        assert_eq!(data["removed_truncated"], false);
        assert_eq!(
            removal("removeChild", &body, false).unwrap()["removed_nodes"],
            LARGE_REMOVAL + 1
        );
        assert!(removal("removeChild", &body.first_child().unwrap(), false).is_none());
        assert!(body.first_child().is_some());
    }

    #[test]
    fn scan_is_bounded_and_does_not_include_page_text_or_attributes() {
        let (_document, body) = tree(SCAN_LIMIT + 5);
        body.set_attribute("id", "private-id");
        body.append_child(NodeHandle::text("private-page-text"));
        let data = removal("innerHTML", &body, true).unwrap();
        assert_eq!(data["removed_nodes"], SCAN_LIMIT);
        assert_eq!(data["removed_truncated"], true);
        assert!(!data.to_string().contains("private"));
        let detached = NodeHandle::element("div");
        for _ in 0..LARGE_REMOVAL {
            detached.append_child(NodeHandle::element("span"));
        }
        assert!(removal("textContent", &detached, true).is_none());
    }

    #[test]
    fn vm_caller_frames_are_bounded_and_keep_author_source_positions() {
        use boa_engine::{NativeFunction, Source, js_string};
        let mut context = Context::default();
        context
            .register_global_builtin_callable(
                js_string!("probe"),
                0,
                NativeFunction::from_copy_closure(|_, _, context| {
                    Ok(boa_engine::JsString::from(frames(context).to_string()).into())
                }),
            )
            .unwrap();
        let result = context
            .eval(
                Source::from_bytes(
                    "function nest(n) { return n ? nest(n - 1) : probe(); } nest(20);",
                )
                .with_path(std::path::Path::new(
                    "https://user:secret@example.test/script.js?token=private#private",
                )),
            )
            .unwrap();
        let rows: serde_json::Value =
            serde_json::from_str(&result.as_string().unwrap().to_std_string_escaped()).unwrap();
        let rows = rows.as_array().unwrap();
        assert_eq!(rows.len(), FRAME_LIMIT);
        assert!(
            rows.iter()
                .all(|row| row["source"] == "https://example.test/script.js")
        );
        assert!(
            rows.iter()
                .all(|row| row["line"].as_u64().is_some_and(|line| line > 0))
        );
        assert!(!serde_json::to_string(rows).unwrap().contains("private"));
    }

    #[test]
    fn native_bulk_removal_and_moves_preserve_their_dom_results() {
        let cases = [
            ("subject.textContent = ''", 0, false, true),
            ("subject.innerHTML = ''", 0, false, true),
            (
                "subject.parentNode.removeChild(subject)",
                LARGE_REMOVAL,
                false,
                false,
            ),
            (
                "document.createDocumentFragment().appendChild(subject)",
                LARGE_REMOVAL,
                false,
                false,
            ),
            (
                "document.createDocumentFragment().insertBefore(subject, null)",
                LARGE_REMOVAL,
                false,
                false,
            ),
            ("dest.appendChild(subject)", LARGE_REMOVAL, true, true),
            ("document.open()", LARGE_REMOVAL, false, false),
        ];
        for (operation, children, retained, connected) in cases {
            let html = format!(
                "<body><div id='subject'>{}</div><div id='dest'></div></body>",
                "<span></span>".repeat(LARGE_REMOVAL)
            );
            let document = crate::html::TreeBuilder::parse(&html).document();
            let mut runtime = super::super::JsRuntime::with_document(document).unwrap();
            let code = format!(
                "const subject = document.getElementById('subject'); const dest = document.getElementById('dest'); {operation}; JSON.stringify({{children:subject.childNodes.length, spans:document.querySelectorAll('span').length, connected:subject.isConnected}})"
            );
            let result = runtime.eval(&code).unwrap();
            let data: serde_json::Value =
                serde_json::from_str(&result.as_string().unwrap().to_std_string_escaped()).unwrap();
            assert_eq!(data["children"], children, "{operation}");
            assert_eq!(
                data["spans"],
                if retained { LARGE_REMOVAL } else { 0 },
                "{operation}"
            );
            assert_eq!(data["connected"], connected, "{operation}");
        }
    }

    #[test]
    fn source_labels_remove_credentials_queries_fragments_and_local_paths() {
        assert_eq!(
            source_label("https://user:secret@example.test/script.js?token=private#private"),
            "https://example.test/script.js"
        );
        assert_eq!(source_label("/private/local.js"), "<anonymous-or-local>");
        assert_eq!(
            source_label("data:text/javascript,private"),
            "<anonymous-or-local>"
        );
    }
}
