//! Transition activity queries must ignore retained, inactive style baselines.
use super::*;

fn style(property: &str, opacity: f32, width: f32) -> PropertyMap {
    let mut values = PropertyMap::new();
    values.insert("opacity", ComputedValue::Number(opacity));
    values.insert("width", ComputedValue::Px(width));
    for (name, value) in [
        ("transition-property", property),
        ("transition-duration", "1s"),
        ("transition-delay", "0s"),
        ("transition-timing-function", "linear"),
    ] {
        values.insert(name, ComputedValue::Keyword(value.into()));
    }
    values
}

#[test]
fn inactive_transition_baselines_do_not_get_scanned_by_activity_queries() {
    let mut timeline = TransitionTimeline::default();
    for node in 0..1024 {
        timeline.sample(node, &mut style("none", 0.0, 10.0));
    }
    let before = timeline.activity_scan_visits.get();
    for _ in 0..16 {
        assert!(!timeline.running_transitions_require_layout());
    }
    assert_eq!(timeline.activity_scan_visits.get(), before);
    assert_eq!(timeline.elements.len(), 1024, "retain every base snapshot");
    timeline.sample(0, &mut style("opacity", 1.0, 10.0));
    assert!(!timeline.running_transitions_require_layout());
    assert_eq!(
        timeline.activity_scan_visits.get(),
        before + 1,
        "visit only the active node"
    );
}

fn assert_queries_match_states(timeline: &TransitionTimeline) {
    let expected: BTreeSet<_> = timeline
        .elements
        .iter()
        .filter_map(|(id, state)| (!state.running.is_empty()).then_some(*id))
        .collect();
    assert_eq!(
        timeline
            .running_node_ids()
            .into_iter()
            .collect::<BTreeSet<_>>(),
        expected
    );
    assert_eq!(timeline.has_running_transitions(), !expected.is_empty());
    let layout = timeline.elements.values().any(|state| {
        state
            .running
            .keys()
            .any(|property| animated_property_requires_layout(property))
    });
    assert_eq!(timeline.running_transitions_require_layout(), layout);
    let expected_scopes: std::collections::BTreeMap<_, _> = timeline
        .elements
        .iter()
        .filter_map(|(id, state)| {
            state
                .running
                .keys()
                .map(|p| transition_cache_scope(p))
                .max()
                .map(|scope| (*id, scope))
        })
        .collect();
    assert_eq!(
        timeline
            .running_cache_scopes()
            .into_iter()
            .collect::<std::collections::BTreeMap<_, _>>(),
        expected_scopes
    );
    let next = timeline.now_ms + 1.0;
    assert_eq!(timeline.sample_will_advance(next), !expected.is_empty());
    assert_eq!(
        timeline.sample_needs_node_lookup(next),
        !expected.is_empty()
            && expected_scopes
                .values()
                .all(|scope| *scope == TransitionCacheScope::Subtree)
    );
}

#[test]
fn transition_activity_follows_start_reversal_cancel_completion_and_detachment() {
    let mut timeline = TransitionTimeline::default();
    for node in 0..64 {
        timeline.sample(node, &mut style("all", 0.0, 10.0));
    }
    assert_queries_match_states(&timeline);
    timeline.sample(1, &mut style("all", 1.0, 10.0));
    timeline.sample(2, &mut style("all", 0.0, 30.0));
    assert_queries_match_states(&timeline);
    assert!(timeline.running_transitions_require_layout());
    timeline.set_time_ms(250.0);
    timeline.sample(1, &mut style("all", 0.0, 10.0));
    assert_queries_match_states(&timeline);
    timeline.sample(2, &mut style("none", 0.0, 30.0));
    assert_queries_match_states(&timeline);
    assert!(!timeline.running_transitions_require_layout());
    timeline.set_time_ms(2000.0);
    timeline.sample(1, &mut style("all", 0.0, 10.0));
    assert_queries_match_states(&timeline);
    assert!(!timeline.has_running_transitions());
    timeline.sample(3, &mut style("all", 1.0, 20.0));
    timeline.cancel_detached_transitions(&[0, 1, 2].into_iter().collect());
    assert_queries_match_states(&timeline);
    assert!(
        timeline.elements.contains_key(&63),
        "cancellation retains inactive baselines"
    );
    timeline.sample(4, &mut style("all", 1.0, 20.0));
    timeline.retain_nodes(&[0, 1].into_iter().collect());
    assert_queries_match_states(&timeline);
    assert_eq!(timeline.elements.len(), 2);
    assert!(!timeline.has_running_transitions());
}
