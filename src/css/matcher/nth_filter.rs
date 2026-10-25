//! Filtered sibling positions, cached only for one unchanged selector pass.

use super::*;

#[derive(Debug, PartialEq, Eq, Hash)]
pub(super) struct FilterKey {
    parent: usize,
    selectors: Vec<Selector>,
    scope: Option<usize>,
    allow_scope_ancestor_escape: bool,
}

pub(super) fn matches(
    node: &NodeHandle,
    argument: &str,
    selectors: &[Selector],
    reverse: bool,
    cache: &mut SelectorMatchCache,
    scope_root: Option<&NodeHandle>,
    allow_scope_ancestor_escape: bool,
) -> bool {
    let Some(formula) = parse_an_plus_b(argument) else {
        return false;
    };
    let position = filtered_position(
        node,
        selectors,
        cache,
        scope_root,
        allow_scope_ancestor_escape,
    );
    position.is_some_and(|(index, total)| {
        formula.matches(if reverse { total - index + 1 } else { index })
    })
}

fn filtered_position(
    node: &NodeHandle,
    selectors: &[Selector],
    cache: &mut SelectorMatchCache,
    scope_root: Option<&NodeHandle>,
    allow_scope_ancestor_escape: bool,
) -> Option<(usize, usize)> {
    let Some(parent) = node.parent_node() else {
        let mut filter_cache = SelectorMatchCache::default();
        return selectors
            .iter()
            .any(|selector| {
                matches_selector_with_scope_mode_cached(
                    node,
                    selector,
                    None,
                    &mut filter_cache,
                    scope_root,
                    allow_scope_ancestor_escape,
                )
            })
            .then_some((1, 1));
    };
    let key = FilterKey {
        parent: parent.identity(),
        selectors: selectors.to_vec(),
        scope: scope_root.map(NodeHandle::identity),
        allow_scope_ancestor_escape,
    };
    if !cache.filtered_positions.contains_key(&key) {
        // A separate unvisited cache prevents history from affecting sibling
        // indices. The key owns selector values: temporary shadow selector
        // clones can reuse addresses during the same style pass.
        let mut filter_cache = SelectorMatchCache::default();
        let children: Vec<_> = cache
            .positions(&parent)
            .elements
            .iter()
            .filter_map(WeakNodeHandle::upgrade)
            .collect();
        let selected: Vec<_> = children
            .iter()
            .filter(|child| {
                selectors.iter().any(|selector| {
                    matches_selector_with_scope_mode_cached(
                        child,
                        selector,
                        None,
                        &mut filter_cache,
                        scope_root,
                        allow_scope_ancestor_escape,
                    )
                })
            })
            .map(NodeHandle::identity)
            .collect();
        let total = selected.len();
        let positions: HashMap<_, _> = selected
            .into_iter()
            .enumerate()
            .map(|(index, id)| (id, (index + 1, total)))
            .collect();
        let position = positions.get(&node.identity()).copied();
        cache.filtered_positions.insert(key, positions);
        return position;
    }
    cache
        .filtered_positions
        .get(&key)?
        .get(&node.identity())
        .copied()
}
