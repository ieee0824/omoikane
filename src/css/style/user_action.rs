//! Conservative subjects whose dynamic flags can affect registered selectors.
use super::*;

const MAX_OBSERVERS: usize = 4096;

// Keep this narrower than animation sampling. Changing filter/backdrop-filter
// from `none` establishes a containing block for fixed descendants, while
// opacity and background position are consumed without changing geometry.
fn user_action_property_requires_layout(property: &str) -> bool {
    !matches!(
        property,
        "opacity" | "background-position-x" | "background-position-y"
    )
}

#[derive(Debug, Default)]
pub(super) struct UserActionDependencies {
    states: HashMap<&'static str, Observers>,
}

#[derive(Debug, Default)]
struct Observers {
    uncertain: bool,
    subjects: Vec<Observer>,
}

#[derive(Debug)]
struct Observer {
    selector: Selector,
    requires_layout: bool,
    nonlocal: bool,
    parent_subtree: bool,
    // A subject whose match can change is strictly below the state node.
    subject_is_descendant: bool,
}

/// Bounded owner-visible result of a user-action cache invalidation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UserActionStyleInvalidation {
    Unchanged,
    Local,
    LocalPaintOnly,
    Full(&'static str),
}

impl UserActionStyleInvalidation {
    pub(crate) fn changed(self) -> bool {
        self != Self::Unchanged
    }
    pub(crate) fn is_full(self) -> bool {
        matches!(self, Self::Full(_))
    }
    pub(crate) fn preserves_layout(self) -> bool {
        matches!(self, Self::LocalPaintOnly)
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Unchanged => "unchanged",
            Self::Local => "local_user_action",
            Self::LocalPaintOnly => "local_user_action_paint_only",
            Self::Full(reason) => reason,
        }
    }
}

fn state_name(state: &str) -> Option<&'static str> {
    match state {
        "hover" => Some("hover"),
        "active" => Some("active"),
        "focus" => Some("focus"),
        "focus-visible" => Some("focus-visible"),
        "focus-within" => Some("focus-within"),
        _ => None,
    }
}

fn tokens_observe_state(source: &str, state: &str) -> bool {
    let Ok(tokens) = super::super::tokenizer::tokenize(source) else {
        return true;
    };
    let tokens: Vec<_> = tokens
        .iter()
        .filter(|token| **token != CssToken::Whitespace)
        .collect();
    tokens.windows(2).any(|pair| {
        matches!(pair, [CssToken::Colon, CssToken::Ident(name)] if name.eq_ignore_ascii_case(state))
    })
}

impl Observers {
    fn unbounded(&mut self) {
        self.uncertain = true;
        self.subjects.clear();
    }

    fn subject(&mut self, part: &SelectorPart, nested: bool, requires_layout: bool) {
        if self.uncertain {
            return;
        }
        if nested || self.subjects.len() == MAX_OBSERVERS {
            self.unbounded();
            return;
        }
        // Ignore combinators and state/structural pseudos. This is a superset
        // of possible subjects, including old and new states of every flag.
        let simples = part
            .simples
            .iter()
            .filter(|simple| {
                matches!(
                    simple,
                    SimpleSelector::Type(_)
                        | SimpleSelector::Universal
                        | SimpleSelector::Class(_)
                        | SimpleSelector::Id(_)
                        | SimpleSelector::Attribute { .. }
                )
            })
            .cloned()
            .collect();
        self.subjects.push(Observer {
            selector: Selector {
                parts: vec![SelectorPart {
                    combinator: None,
                    simples,
                }],
            },
            requires_layout,
            nonlocal: false,
            parent_subtree: false,
            subject_is_descendant: false,
        });
    }

    fn selector(&mut self, selector: &Selector, state: &str, nested: bool, requires_layout: bool) {
        for (index, part) in selector.parts.iter().enumerate() {
            let previous_subjects = self.subjects.len();
            if part.simples.iter().any(|simple| {
                matches!(simple, SimpleSelector::PseudoClass(name) if name.eq_ignore_ascii_case(state))
            }) {
                self.subject(part, nested, requires_layout);
            }
            for simple in &part.simples {
                match simple {
                    SimpleSelector::Is(selectors)
                    | SimpleSelector::Where(selectors)
                    | SimpleSelector::Not(selectors) => {
                        // Boolean lists test the same candidate. Inner sibling
                        // and relational dependencies keep their own guards.
                        for selector in selectors {
                            self.selector(selector, state, nested, requires_layout);
                        }
                    }
                    SimpleSelector::Has(selectors) => {
                        let arguments_start = self.subjects.len();
                        for relative in selectors {
                            self.selector(&relative.selector, state, nested, requires_layout);
                        }
                        self.mark_new_nonlocal(arguments_start);
                    }
                    SimpleSelector::PseudoClass(name) | SimpleSelector::PseudoElement(name)
                        if name.contains('(') && tokens_observe_state(name, state) =>
                    {
                        self.unbounded()
                    }
                    _ => {}
                }
            }
            self.extend_subjects(previous_subjects, &selector.parts[index + 1..]);
        }
    }

    fn extend_subjects(&mut self, start: usize, suffix: &[SelectorPart]) {
        let Some(observers) = self.subjects.get_mut(start..) else {
            return;
        };
        for observer in observers {
            // Only a sibling step at the changed node's level can leave its
            // subtree. Once descended, later sibling steps remain inside it.
            // Nested boolean selectors retain the relationship already proven for
            // their candidate. A nonlocal inner dependency cannot be repaired
            // by a later descendant step.
            for part in suffix {
                match part.combinator {
                    Some(Combinator::Child | Combinator::Descendant) => {
                        observer.subject_is_descendant = true;
                    }
                    Some(Combinator::AdjacentSibling | Combinator::GeneralSibling) => {
                        observer.parent_subtree |= !observer.subject_is_descendant;
                    }
                    None => observer.nonlocal = true,
                }
            }
        }
    }

    fn mark_new_nonlocal(&mut self, start: usize) {
        // An uncertain nested selector may have cleared the observer list.
        if let Some(observers) = self.subjects.get_mut(start..) {
            for observer in observers {
                observer.nonlocal = true;
            }
        }
    }

    fn rules(&mut self, rules: &[Rule], state: &str, nested: bool) {
        for rule in rules {
            match rule {
                Rule::Style(rule) => {
                    let requires_layout = !rule.rules.is_empty()
                        || rule.declarations.iter().any(|declaration| {
                            user_action_property_requires_layout(&declaration.name)
                        });
                    for selector in &rule.selectors {
                        self.selector(selector, state, nested, requires_layout);
                    }
                    self.rules(&rule.rules, state, true);
                }
                Rule::At(rule) => {
                    if rule.name.eq_ignore_ascii_case("scope")
                        && tokens_observe_state(&rule.prelude, state)
                    {
                        self.unbounded();
                    }
                    if let Some(rules) = &rule.block {
                        self.rules(rules, state, nested);
                    }
                }
                Rule::FontFace(_) => {}
            }
        }
    }
}

impl UserActionDependencies {
    pub(super) fn clear(&mut self) {
        self.states.clear();
    }

    fn invalidation_root(
        &mut self,
        stylesheets: &[StylesheetInput],
        node: &NodeHandle,
        state: &str,
    ) -> Option<NodeHandle> {
        if !self.affects(stylesheets, node, state) {
            return None;
        }
        let needs_parent = state_name(state)
            .and_then(|name| self.states.get(name))
            .is_some_and(|observers| {
                observers.subjects.iter().any(|observer| {
                    observer.parent_subtree
                        && super::super::matcher::matches_selector(node, &observer.selector)
                })
            });
        Some(if needs_parent {
            sibling_invalidation_parent(node).unwrap_or_else(|| node.clone())
        } else {
            node.clone()
        })
    }

    fn matching_rules_require_layout(&self, node: &NodeHandle, state: &str) -> bool {
        state_name(state)
            .and_then(|name| self.states.get(name))
            .is_none_or(|observers| {
                observers.uncertain
                    || observers.subjects.iter().any(|observer| {
                        observer.requires_layout
                            && super::super::matcher::matches_selector(node, &observer.selector)
                    })
            })
    }

    fn affects(&mut self, stylesheets: &[StylesheetInput], node: &NodeHandle, state: &str) -> bool {
        let Some(state) = state_name(state) else {
            return true;
        };
        let observers = self.states.entry(state).or_insert_with(|| {
            let mut observers = Observers::default();
            for input in stylesheets {
                observers.rules(&input.stylesheet.rules, state, false);
            }
            observers
        });
        observers.uncertain
            || observers
                .subjects
                .iter()
                .any(|observer| super::super::matcher::matches_selector(node, &observer.selector))
    }
}

// Sibling steps from a changed node stay below its element parent. Keep the
// document-root and shadow-root boundaries on the conservative full path.
fn sibling_invalidation_parent(node: &NodeHandle) -> Option<NodeHandle> {
    let parent = node.parent_node()?;
    (parent.node_type() == NodeType::Element
        && parent
            .parent_node()
            .is_some_and(|ancestor| ancestor.node_type() == NodeType::Element))
    .then_some(parent)
}

impl StyleResolver {
    /// Invalidates only changed state subtrees when selector dependencies prove
    /// that affected subjects stay inside them. Sibling dependencies expand the
    /// invalidation to the element parent's subtree; ancestors remain guarded.
    /// Geometry is still
    /// invalidated by the owner; uncertain selectors retain full invalidation.
    /// The locality proof assumes inheritance only from the flat-tree parent,
    /// ancestor-only selector traversal, and global clearing on shared resolver
    /// context changes. Shadow/slot, relational selectors, transitions and
    /// animations that can affect the changed branch or geometry fall back.
    /// New cross-element style dependencies must extend these guards before
    /// retaining caches (for example future anchor positioning).
    pub(crate) fn invalidate_user_action_styles(
        &mut self,
        changes: &[(NodeHandle, &'static str)],
    ) -> UserActionStyleInvalidation {
        use UserActionStyleInvalidation::{Full, Local, LocalPaintOnly, Unchanged};
        if !self.invalidate_user_action_selectors(changes) {
            return Unchanged;
        }
        let roots = changes
            .iter()
            .filter_map(|(node, state)| {
                self.user_action_dependencies
                    .invalidation_root(&self.stylesheets, node, state)
            })
            .collect::<Vec<_>>();
        let requires_layout = changes.iter().any(|(node, state)| {
            self.user_action_dependencies
                .matching_rules_require_layout(node, state)
        });
        let fallback = if self.has_running_transitions() {
            Some("fallback_active_transition")
        } else if roots.iter().any(|node| {
            node.parent_node()
                .is_some_and(|parent| parent.node_type() == NodeType::Document)
        }) {
            Some("fallback_document_root")
        } else {
            self.user_action_fallback(changes)
        };
        if let Some(reason) = fallback {
            self.invalidate_style_cache();
            return Full(reason);
        }
        let Some(identities) = invalidation_subtrees(roots.iter().cloned()) else {
            self.invalidate_style_cache();
            return Full("fallback_shadow_or_slot");
        };
        if !self.animations_allow_user_action_subtree_invalidation(&identities, &roots) {
            self.invalidate_style_cache();
            return Full("fallback_active_animation");
        }
        self.cache
            .retain(|identity, _| !identities.contains(identity));
        self.pseudo_cache
            .retain(|key, _| !identities.contains(&key.0));
        if requires_layout {
            Local
        } else {
            LocalPaintOnly
        }
    }

    fn user_action_fallback(&self, changes: &[(NodeHandle, &'static str)]) -> Option<&'static str> {
        for (node, state) in changes {
            let Some(observers) =
                state_name(state).and_then(|name| self.user_action_dependencies.states.get(name))
            else {
                return Some("fallback_uncertain_selector");
            };
            if observers.uncertain {
                return Some("fallback_uncertain_selector");
            }
            if observers.subjects.iter().any(|observer| {
                (observer.nonlocal
                    || (observer.parent_subtree && sibling_invalidation_parent(node).is_none()))
                    && super::super::matcher::matches_selector(node, &observer.selector)
            }) {
                return Some("fallback_nonlocal_selector");
            }
        }
        None
    }

    /// Drops dynamic selector matches and reports whether any changed flag can
    /// affect a registered rule. False preserves computed styles and geometry;
    /// unknown selector contexts conservatively return true. Node handles are
    /// borrowed only for this call and are never retained in the index.
    pub(crate) fn invalidate_user_action_selectors(
        &mut self,
        changes: &[(NodeHandle, &'static str)],
    ) -> bool {
        self.selector_match_cache = SelectorMatchCache::default();
        changes.iter().any(|(node, state)| {
            self.user_action_dependencies
                .affects(&self.stylesheets, node, state)
        })
    }
}

#[cfg(test)]
mod tests;
