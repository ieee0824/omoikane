use super::*;

#[test]
fn adjacent_relational_and_nth_selectors_share_one_sibling_index() {
    for count in [32, 128, 512] {
        let parent = NodeHandle::element("main");
        let nodes: Vec<_> = (0..count)
            .map(|_| {
                parent.append_child(NodeHandle::text("between"));
                let node = NodeHandle::element("i");
                parent.append_child(node.clone());
                parent.append_child(NodeHandle::comment("between"));
                node
            })
            .collect();
        let selectors =
            super::super::parse_selector_list("i + i, i:has(+ i), i:nth-child(2n), i ~ i").unwrap();
        let mut cache = SelectorMatchCache::default();
        for (index, node) in nodes.iter().enumerate() {
            for (selector, expected) in
                selectors
                    .iter()
                    .zip([index > 0, index + 1 < count, index % 2 == 1, index > 0])
            {
                assert_eq!(
                    matches_selector_cached(node, selector, &mut cache),
                    expected
                );
            }
        }
        assert_eq!(cache.structural_builds, 1);
    }
}

#[test]
fn cached_neighbors_preserve_non_element_and_edge_positions() {
    let parent = NodeHandle::document_fragment();
    let first = NodeHandle::element("i");
    let text = NodeHandle::text("text");
    let last = NodeHandle::xml_element("Item", Some("urn:example".into()));
    parent.append_child(NodeHandle::comment("before"));
    parent.append_child(first.clone());
    parent.append_child(text.clone());
    parent.append_child(NodeHandle::comment("after text"));
    parent.append_child(last.clone());
    let mut cache = SelectorMatchCache::default();
    assert_eq!(previous_element_sibling(&first, &mut cache), None);
    assert_eq!(next_element_sibling(&first, &mut cache), Some(last.clone()));
    assert_eq!(
        previous_element_sibling(&text, &mut cache),
        Some(first.clone())
    );
    assert_eq!(next_element_sibling(&text, &mut cache), Some(last.clone()));
    assert_eq!(
        following_element_siblings(&text, &mut cache),
        vec![last.clone()]
    );
    assert_eq!(previous_element_sibling(&last, &mut cache), Some(first));
    assert_eq!(next_element_sibling(&last, &mut cache), None);
    assert_eq!(cache.structural_builds, 1);
}

#[test]
fn sibling_index_does_not_retain_removed_nodes() {
    let parent = NodeHandle::element("main");
    let child = NodeHandle::element("i");
    parent.append_child(child.clone());
    let weak = child.downgrade();
    let mut cache = SelectorMatchCache::default();
    assert_eq!(previous_element_sibling(&child, &mut cache), None);
    parent.remove_child(&child).unwrap();
    drop(child);
    assert!(
        weak.upgrade().is_none(),
        "a cached sibling index must not own DOM nodes"
    );
}
