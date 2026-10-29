use super::*;

#[test]
fn container_dependency_queries_do_not_rescan_unchanged_style_caches() {
    let mut resolver = StyleResolver::new();
    let before = resolver.container_context_scan_count();
    for _ in 0..16 {
        assert!(!resolver.needs_container_contexts());
    }
    assert_eq!(
        resolver.container_context_scan_count(),
        before + 1,
        "repeated queries without cache changes need only one full dependency scan"
    );

    let node = NodeHandle::element("div");
    resolver.computed_style_shared(&node);
    assert!(!resolver.needs_container_contexts());
    assert_eq!(resolver.container_context_scan_count(), before + 2);
    assert!(!resolver.needs_container_contexts());
    assert_eq!(resolver.container_context_scan_count(), before + 2);

    resolver.add_stylesheet(
        Origin::Author,
        crate::css::parse_stylesheet("@container (min-width:1px){div{color:red}}").unwrap(),
    );
    assert!(resolver.needs_container_contexts());
    assert_eq!(resolver.container_context_scan_count(), before + 3);
}

#[test]
fn non_color_container_units_request_container_contexts() {
    let node = NodeHandle::element("div");
    node.set_attribute("style", "tab-size: calc(10 + (sign(2cqw - 10px) * 5))");
    let mut resolver = StyleResolver::new();

    resolver.computed_style_shared(&node);

    assert!(resolver.needs_container_contexts());
}
