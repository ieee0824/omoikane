use super::*;

#[test]
fn all_slot_accessors_and_layout_share_one_linear_host_scan() {
    for (slot_count, child_count) in [(1, 128), (16, 32), (64, 128), (128, 128)] {
        let host = NodeHandle::element("div");
        let root = host.attach_shadow(ShadowRootMode::Open).unwrap();
        let slots: Vec<_> = (0..slot_count)
            .map(|index| {
                let slot = NodeHandle::element("slot");
                slot.set_attribute("name", format!("s{index}"));
                root.append_child(slot.clone());
                slot
            })
            .collect();
        let children: Vec<_> = (0..child_count)
            .map(|index| {
                let child = NodeHandle::element("i");
                child.set_attribute("slot", format!("s{}", index % slot_count));
                host.append_child(child.clone());
                child
            })
            .collect();
        SLOT_ASSIGNMENT_WORK.with(|work| work.set((0, 0, 0)));
        for _ in 0..3 {
            for (index, slot) in slots.iter().enumerate() {
                let expected: Vec<_> = children
                    .iter()
                    .skip(index)
                    .step_by(slot_count)
                    .cloned()
                    .collect();
                assert_eq!(slot.assigned_nodes(false), expected);
            }
            for (index, child) in children.iter().enumerate() {
                assert_eq!(
                    child.assigned_slot(),
                    Some(slots[index % slot_count].clone())
                );
            }
            assert_eq!(host.layout_child_nodes().len(), child_count);
        }
        let work = SLOT_ASSIGNMENT_WORK.with(std::cell::Cell::get);
        eprintln!(
            "slots={slot_count} children={child_count} builds={} shadow_visits={} light_visits={}",
            work.0, work.1, work.2
        );
        assert_eq!(work, (1, slot_count, child_count));
    }
}

#[test]
fn warm_assignments_follow_xml_attributes_removal_and_slot_reordering() {
    let host = NodeHandle::element("div");
    let child = NodeHandle::element("i");
    host.append_child(child.clone());
    let root = host.attach_shadow(ShadowRootMode::Open).unwrap();
    let first = NodeHandle::element("slot");
    let second = NodeHandle::element("slot");
    root.append_child(first.clone());
    root.append_child(second.clone());
    assert_eq!(child.assigned_slot(), Some(first.clone()));
    root.insert_before(second.clone(), &first).unwrap();
    assert_eq!(child.assigned_slot(), Some(second.clone()));
    first.set_xml_attribute_ns("name", None, "name", "named");
    child.set_xml_attribute("slot", "named");
    assert_eq!(child.assigned_slot(), Some(first.clone()));
    first.remove_xml_attribute("name");
    assert_eq!(child.assigned_slot(), None);
    child.remove_xml_attribute("slot");
    assert_eq!(child.assigned_slot(), Some(second.clone()));
    second.set_attribute("NAME", "other");
    assert_eq!(child.assigned_slot(), Some(first.clone()));
    second.remove_attribute("NAME");
    assert_eq!(child.assigned_slot(), Some(second.clone()));
    host.remove_child(&child).unwrap();
    assert!(second.assigned_nodes(false).is_empty());
    host.append_child(child.clone());
    assert_eq!(second.assigned_nodes(false), vec![child]);
}

#[test]
fn assignment_snapshots_do_not_keep_removed_nodes_or_hosts_alive() {
    let host = NodeHandle::element("div");
    let child = NodeHandle::element("i");
    let weak_child = child.downgrade();
    host.append_child(child.clone());
    let root = host.attach_shadow(ShadowRootMode::Open).unwrap();
    let slot = NodeHandle::element("slot");
    root.append_child(slot.clone());
    assert_eq!(slot.assigned_nodes(false), vec![child.clone()]);
    host.remove_child(&child).unwrap();
    drop(child);
    assert!(!weak_child.is_alive());
    let weak_slot = slot.downgrade();
    root.remove_child(&slot).unwrap();
    drop(slot);
    assert!(!weak_slot.is_alive());

    let child = NodeHandle::element("i");
    let slot = NodeHandle::element("slot");
    host.append_child(child.clone());
    root.append_child(slot.clone());
    assert_eq!(slot.assigned_nodes(false), vec![child.clone()]);
    drop(host);
    // Host destruction is not a mutation; surviving handles still lose assignment.
    assert_eq!(child.assigned_slot(), None);
    assert!(slot.assigned_nodes(false).is_empty());
}

#[test]
fn moving_light_and_shadow_subtrees_between_hosts_updates_both_snapshots() {
    let first = NodeHandle::element("div");
    let second = NodeHandle::element("div");
    let first_root = first.attach_shadow(ShadowRootMode::Open).unwrap();
    let second_root = second.attach_shadow(ShadowRootMode::Closed).unwrap();
    let container = NodeHandle::element("section");
    let slot = NodeHandle::element("slot");
    container.append_child(slot.clone());
    first_root.append_child(container.clone());
    let a = NodeHandle::text("first");
    let b = NodeHandle::text("second");
    first.append_child(a.clone());
    second.append_child(b.clone());
    assert_eq!(slot.assigned_nodes(false), vec![a.clone()]);
    second_root.append_child(container);
    assert_eq!(a.assigned_slot(), None);
    assert_eq!(slot.assigned_nodes(false), vec![b.clone()]);
    second.insert_before(a.clone(), &b).unwrap();
    assert_eq!(slot.assigned_nodes(false), vec![a, b]);
}
