//! Conservative computed-style cache reach for ordinary DOM child-list changes.
use super::*;

/// Whether selectors can observe a child-list change outside the changed parents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TreeMutationReach {
    /// Only the changed parents, their children and those descendants.
    ParentSubtree,
    /// A selector such as `:has()` can restyle ancestors or unrelated elements.
    NonLocal,
}

/// Raw pseudo-class text is kept for unrecognized functional forms; treat any
/// mention of `has(` there as relational rather than proving it inert.
fn simple_observes_descendants(simple: &SimpleSelector) -> bool {
    match simple {
        SimpleSelector::Has(_) => true,
        SimpleSelector::PseudoClass(name) | SimpleSelector::PseudoElement(name) => {
            name.to_ascii_lowercase().contains("has(")
        }
        SimpleSelector::Is(selectors)
        | SimpleSelector::Where(selectors)
        | SimpleSelector::Not(selectors)
        | SimpleSelector::NthChild { selectors, .. } => {
            selectors.iter().any(selector_observes_descendants)
        }
        _ => false,
    }
}

fn selector_observes_descendants(selector: &Selector) -> bool {
    selector
        .parts
        .iter()
        .any(|part| part.simples.iter().any(simple_observes_descendants))
}

fn rules_observe_descendants(rules: &[Rule]) -> bool {
    rules.iter().any(|rule| match rule {
        Rule::Style(rule) => {
            rule.selectors.iter().any(selector_observes_descendants)
                || rules_observe_descendants(&rule.rules)
        }
        Rule::At(rule) => {
            (rule.name.eq_ignore_ascii_case("scope")
                && rule.prelude.to_ascii_lowercase().contains("has("))
                || rule.block.as_deref().is_some_and(rules_observe_descendants)
        }
        Rule::FontFace(_) => false,
    })
}

/// A parent at or above the document element owns every style in the document.
fn is_local_parent(parent: &NodeHandle) -> bool {
    parent.is_html_element()
        && parent.prefix().is_none()
        && parent
            .parent_node()
            .is_some_and(|grandparent| grandparent.node_type() == NodeType::Element)
}

impl StyleResolver {
    fn tree_mutation_reach(&mut self) -> TreeMutationReach {
        if let Some(reach) = self.tree_mutation_reach {
            return reach;
        }
        let reach = if self
            .stylesheets
            .iter()
            .any(|input| rules_observe_descendants(&input.stylesheet.rules))
        {
            TreeMutationReach::NonLocal
        } else {
            TreeMutationReach::ParentSubtree
        };
        self.tree_mutation_reach = Some(reach);
        reach
    }

    /// Discards computed styles reachable from an ordinary child-list change.
    ///
    /// `parents` are the connected elements whose child lists changed, and
    /// `moved` are the nodes inserted into or removed from them. Without
    /// `:has()`, selectors observe such a change only through a parent
    /// (`:empty`), its children (structural pseudo-classes and sibling
    /// combinators) and their descendants (inheritance and descendant
    /// combinators), so other cached styles are retained.
    ///
    /// A running animation on an ancestor of those subtrees clears all styles.
    ///
    /// Returns a bounded diagnostic outcome: `"local_tree_subtree"` retained
    /// styles outside those subtrees, and every `"fallback_*"` outcome cleared
    /// all computed and pseudo styles. Selectors are always invalidated, and the
    /// caller still invalidates geometry.
    pub(crate) fn invalidate_tree_mutation_subtrees(
        &mut self,
        parents: &[NodeHandle],
        moved: &[NodeHandle],
    ) -> &'static str {
        self.selector_match_cache = SelectorMatchCache::default();
        let fallback = if self.tree_mutation_reach() == TreeMutationReach::NonLocal {
            Some("fallback_tree_relational_selector")
        } else if parents.is_empty() || !parents.iter().all(is_local_parent) {
            Some("fallback_tree_root_or_namespace")
        } else if self.has_running_transitions() {
            Some("fallback_active_transition")
        } else {
            None
        };
        if let Some(reason) = fallback {
            self.invalidate_style_cache();
            return reason;
        }
        let roots: Vec<_> = parents.iter().chain(moved).cloned().collect();
        let Some(identities) = invalidation_subtrees(roots.iter().cloned()) else {
            self.invalidate_style_cache();
            return "fallback_tree_shadow_or_slot";
        };
        if !self.animations_allow_tree_mutation_invalidation(&roots) {
            self.invalidate_style_cache();
            return "fallback_active_animation";
        }
        self.cache
            .retain(|identity, _| !identities.contains(identity));
        self.pseudo_cache
            .retain(|key, _| !identities.contains(&key.0));
        "local_tree_subtree"
    }
}

#[cfg(test)]
mod tests;
