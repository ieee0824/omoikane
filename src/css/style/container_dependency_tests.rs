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
