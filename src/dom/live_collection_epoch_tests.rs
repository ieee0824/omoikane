use super::*;

fn mask(names: &[&str]) -> Option<u32> {
    Some(names.iter().fold(0, |mask, name| {
        let index = TRACKED_COLLECTION_ATTRIBUTES
            .iter()
            .position(|tracked| tracked == name)
            .expect("tracked attribute");
        mask | (1 << index)
    }))
}

#[test]
fn untracked_attribute_writes_keep_declared_collection_generations() {
    let element = NodeHandle::element("div");
    let all = live_collection_epoch(None);
    let tree_only = live_collection_epoch(mask(&[]));
    let class = live_collection_epoch(mask(&["class"]));
    for index in 0..100 {
        element.set_attribute("style", format!("color: rgb({index}, 0, 0)"));
        element.set_attribute(format!("data-n{}", index % 3), "x");
    }
    element.remove_attribute("style");
    element.set_xml_attribute_ns("p:style", Some("urn:test".into()), "style", "x");
    element.remove_xml_attribute_ns(Some("urn:test"), "style");
    assert_ne!(live_collection_epoch(None), all);
    assert_eq!(live_collection_epoch(mask(&[])), tree_only);
    assert_eq!(live_collection_epoch(mask(&["class"])), class);
}

#[test]
fn tracked_attribute_writes_invalidate_only_collections_that_declare_them() {
    let element = NodeHandle::element("a");
    let observe = || {
        [
            live_collection_epoch(mask(&[])),
            live_collection_epoch(mask(&["class"])),
            live_collection_epoch(mask(&["href"])),
            live_collection_epoch(mask(&["name"])),
            live_collection_epoch(mask(&["class", "href", "name"])),
        ]
    };
    let changed = |before: [u64; 5], after: [u64; 5]| -> Vec<bool> {
        before.iter().zip(after).map(|(a, b)| *a != b).collect()
    };

    let before = observe();
    element.set_attribute("class", "a");
    assert_eq!(
        changed(before, observe()),
        [false, true, false, false, true]
    );
    let before = observe();
    element.set_xml_attribute_ns("href", None, "href", "#x");
    assert_eq!(
        changed(before, observe()),
        [false, false, true, false, true]
    );
    let before = observe();
    element.set_attribute("name", "n");
    assert_eq!(
        changed(before, observe()),
        [false, false, false, true, true]
    );
    // Exact UTF-16 value updates (Attr.value) use the same invalidation.
    let before = observe();
    element.set_attribute_value_ns_utf16(None, "class", &[0x62]);
    assert_eq!(
        changed(before, observe()),
        [false, true, false, false, true]
    );
}

#[test]
fn tree_mutations_invalidate_every_collection_generation() {
    let parent = NodeHandle::element("div");
    let child = NodeHandle::element("span");
    let before = [
        live_collection_epoch(None),
        live_collection_epoch(mask(&[])),
        live_collection_epoch(mask(&["class"])),
    ];
    parent.append_child(child.clone());
    let appended = [
        live_collection_epoch(None),
        live_collection_epoch(mask(&[])),
        live_collection_epoch(mask(&["class"])),
    ];
    assert!(before.iter().zip(appended).all(|(a, b)| *a != b));
    parent.remove_child(&child).unwrap();
    assert_ne!(live_collection_epoch(mask(&[])), appended[1]);
}
