use super::*;

#[test]
fn disabled_nested_legend_ancestor_reads_grow_linearly() {
    for depth in [32, 64, 128] {
        let root = NodeHandle::element("fieldset");
        root.set_attribute("disabled", "");
        let mut current = root.clone();
        for index in 0..depth {
            let legend = NodeHandle::element("legend");
            current.append_child(legend.clone());
            if index + 1 < depth {
                current = NodeHandle::element("fieldset");
                current.set_attribute("disabled", "");
                legend.append_child(current.clone());
            } else {
                current = legend;
            }
        }
        let input = NodeHandle::element("input");
        current.append_child(input.clone());
        reset_child_snapshot_handle_clones();
        PARENT_NODE_READS.with(|work| work.set(0));
        assert!(!is_actually_disabled(&input));
        let reads = PARENT_NODE_READS.with(std::cell::Cell::get);
        assert!(reads <= 2 * depth + 1, "depth {depth} read {reads} parents");
        assert_eq!(child_snapshot_handle_clones(), 0);
    }
}

#[test]
fn disabled_nested_legends_preserve_each_fieldset_exception() {
    let outer = NodeHandle::element("fieldset");
    outer.set_attribute("disabled", "");
    let outer_legend = NodeHandle::element("legend");
    outer.append_child(outer_legend.clone());
    let inner = NodeHandle::element("fieldset");
    inner.set_attribute("disabled", "");
    outer_legend.append_child(inner.clone());
    let inner_legend = NodeHandle::element("legend");
    inner.append_child(inner_legend.clone());
    for tag in ["input", "x-control"] {
        let input = NodeHandle::element(tag);
        if tag == "x-control" {
            input.set_form_associated_custom(true);
        }
        let wrapper = NodeHandle::element("span");
        wrapper.append_child(input.clone());
        inner_legend.append_child(wrapper.clone());
        assert!(!is_actually_disabled(&input));
        input.set_attribute("disabled", "");
        assert!(is_actually_disabled(&input));
        input.remove_attribute("disabled");
        assert!(!is_actually_disabled(&input));
        inner.append_child(wrapper.clone());
        assert!(is_actually_disabled(&input));
        inner_legend.append_child(wrapper);
        assert!(!is_actually_disabled(&input));
        outer.append_child(inner.clone());
        assert!(is_actually_disabled(&input));
        outer_legend.append_child(inner.clone());
        assert!(!is_actually_disabled(&input));
    }
    assert!(is_actually_disabled(&inner));
}

#[test]
fn disabled_options_and_optgroups_ignore_fieldset_inheritance() {
    let fieldset = NodeHandle::element("fieldset");
    fieldset.set_attribute("disabled", "");
    let group = NodeHandle::element("optgroup");
    let option = NodeHandle::element("option");
    group.append_child(option.clone());
    fieldset.append_child(group.clone());
    assert!(!is_actually_disabled(&group));
    assert!(!is_actually_disabled(&option));
    group.set_attribute("disabled", "");
    assert!(is_actually_disabled(&group));
    assert!(is_actually_disabled(&option));
    group.remove_attribute("disabled");
    option.set_attribute("disabled", "");
    assert!(!is_actually_disabled(&group));
    assert!(is_actually_disabled(&option));
}
