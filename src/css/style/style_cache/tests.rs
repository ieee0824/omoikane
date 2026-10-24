use super::*;
use crate::css::{PseudoElement, StyleResolver};

fn style(container_units: bool) -> Arc<ComputedStyle> {
    let mut style = ComputedStyle::default();
    if container_units {
        let declaration = crate::css::parse_style_attribute(
            "color: color(srgb calc(0.5 + (sign(2cqw - 10px) * 0.1)) 0 0)",
        )
        .into_iter()
        .next()
        .unwrap();
        style
            .component_values
            .insert("color".into(), declaration.value);
    }
    Arc::new(style)
}

#[test]
fn container_memo_tracks_replacement_mutable_entries_and_pseudo_removal() {
    let mut resolver = StyleResolver::new();
    resolver.cache.insert(1, style(false));
    assert!(!resolver.needs_container_contexts());
    resolver.cache.insert(1, style(true));
    assert!(
        resolver.needs_container_contexts(),
        "same-key replacement must invalidate false"
    );
    Arc::make_mut(resolver.cache.get_mut(&1).unwrap())
        .component_values
        .clear();
    assert!(
        !resolver.needs_container_contexts(),
        "mutable value access must invalidate true"
    );
    resolver
        .pseudo_cache
        .insert((1, PseudoElement::Before), style(true));
    assert!(resolver.needs_container_contexts());
    resolver.pseudo_cache.retain(|_, _| false);
    assert!(!resolver.needs_container_contexts());
    for cached in resolver.cache.values_mut() {
        Arc::make_mut(cached).component_values = style(true).component_values.clone();
    }
    assert!(resolver.needs_container_contexts());
    resolver.cache.clear();
    assert!(!resolver.needs_container_contexts());
}

#[test]
fn container_memo_distinguishes_replaced_maps_with_equal_revisions() {
    let mut resolver = StyleResolver::new();
    resolver.cache = StyleCache::default();
    resolver.cache.insert(1, style(false));
    assert!(!resolver.needs_container_contexts());
    let mut replacement = StyleCache::default();
    replacement.insert(1, style(true));
    resolver.cache = replacement;
    assert!(resolver.needs_container_contexts());
}

#[test]
fn container_memo_exhausted_revision_never_reuses_a_stale_result() {
    let mut resolver = StyleResolver::new();
    resolver
        .cache
        .epoch
        .store(usize::MAX - 1, Ordering::Relaxed);
    resolver.cache.insert(1, style(true));
    assert!(resolver.needs_container_contexts());
    resolver.cache.insert(1, style(false));
    assert!(!resolver.needs_container_contexts());
    let before = resolver.container_context_scan_count();
    for _ in 0..3 {
        assert!(!resolver.needs_container_contexts());
    }
    assert_eq!(resolver.container_context_scan_count(), before + 3);
}
