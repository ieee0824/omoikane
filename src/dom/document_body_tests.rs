use super::*;

#[test]
fn document_body_uses_direct_child_order_and_legacy_html_namespace() {
    let document = NodeHandle::document();
    document.append_child(NodeHandle::comment("before the root"));
    let root = NodeHandle::element("html");
    document.append_child(root.clone());
    let nested = NodeHandle::element("div");
    nested.append_child(NodeHandle::element("body"));
    root.append_child(nested);
    root.append_child(NodeHandle::xml_element("body", Some("urn:foreign".into())));
    root.append_child(NodeHandle::xml_element("body", None));
    assert_eq!(document.document_body(), None);
    let first = NodeHandle::element("body");
    let frameset = NodeHandle::element("frameset");
    let second = NodeHandle::element("body");
    root.append_child(first.clone());
    root.append_child(frameset.clone());
    root.append_child(second.clone());
    assert_eq!(document.document_body(), Some(first.clone()));
    root.append_child(first.clone());
    assert_eq!(document.document_body(), Some(frameset.clone()));
    root.remove_child(&frameset).unwrap();
    assert_eq!(document.document_body(), Some(second.clone()));
    root.remove_child(&second).unwrap();
    root.remove_child(&first).unwrap();
    assert_eq!(document.document_body(), None);
}

#[test]
fn document_body_preserves_exact_namespace_case_prefix_and_root_identity() {
    const HTML: &str = "http://www.w3.org/1999/xhtml";
    let document = NodeHandle::document();
    let root = NodeHandle::xml_element("h:html", Some(HTML.into()));
    let body = NodeHandle::xml_element("b:body", Some(HTML.into()));
    root.append_child(body.clone());
    document.append_child(root.clone());
    assert_eq!(document.document_body(), Some(body.clone()));
    root.remove_child(&body).unwrap();
    for child in [
        NodeHandle::xml_element("BODY", Some(HTML.into())),
        NodeHandle::xml_element("body", None),
        NodeHandle::html_element_ns("body", "urn:foreign"),
    ] {
        root.append_child(child);
    }
    assert_eq!(document.document_body(), None);
    let frameset = NodeHandle::xml_element("f:frameset", Some(HTML.into()));
    root.append_child(frameset.clone());
    assert_eq!(document.document_body(), Some(frameset));
    for invalid_root in [
        NodeHandle::xml_element("html", None),
        NodeHandle::html_element_ns("HTML", HTML),
        NodeHandle::html_element_ns("html", "urn:foreign"),
        NodeHandle::xml_element("svg", Some("http://www.w3.org/2000/svg".into())),
    ] {
        let invalid_document = NodeHandle::document();
        invalid_root.append_child(NodeHandle::xml_element("body", Some(HTML.into())));
        invalid_document.append_child(invalid_root);
        assert_eq!(invalid_document.document_body(), None);
    }
    assert_eq!(NodeHandle::document().document_body(), None);
    assert_eq!(root.document_body(), None);
}
