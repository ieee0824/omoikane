//! Conservative subjects whose dynamic flags can affect registered selectors.
use super::*;

const MAX_OBSERVERS: usize = 4096;

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
    nonlocal: bool,
}

/// Bounded owner-visible result of a user-action cache invalidation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UserActionStyleInvalidation {
    Unchanged,
    Local,
    Full(&'static str),
}

impl UserActionStyleInvalidation {
    pub(crate) fn changed(self) -> bool {
        self != Self::Unchanged
    }
    pub(crate) fn is_full(self) -> bool {
        matches!(self, Self::Full(_))
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Unchanged => "unchanged",
            Self::Local => "local_user_action",
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

    fn subject(&mut self, part: &SelectorPart, nested: bool) {
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
            nonlocal: false,
        });
    }

    fn selector(&mut self, selector: &Selector, state: &str, nested: bool) {
        let previous_subjects = self.subjects.len();
        for part in &selector.parts {
            if part.simples.iter().any(|simple| {
                matches!(simple, SimpleSelector::PseudoClass(name) if name.eq_ignore_ascii_case(state))
            }) {
                self.subject(part, nested);
            }
            for simple in &part.simples {
                match simple {
                    SimpleSelector::Is(selectors)
                    | SimpleSelector::Where(selectors)
                    | SimpleSelector::Not(selectors) => {
                        for selector in selectors {
                            self.selector(selector, state, nested);
                        }
                    }
                    SimpleSelector::Has(selectors) => {
                        for relative in selectors {
                            self.selector(&relative.selector, state, nested);
                        }
                    }
                    SimpleSelector::PseudoClass(name) | SimpleSelector::PseudoElement(name)
                        if name.contains('(') && tokens_observe_state(name, state) =>
                    {
                        self.unbounded()
                    }
                    _ => {}
                }
            }
        }
        if self.subjects.len() > previous_subjects
            && selector.parts.iter().any(|part| {
                matches!(
                    part.combinator,
                    Some(Combinator::AdjacentSibling | Combinator::GeneralSibling)
                ) || part.simples.iter().any(|simple| {
                    matches!(
                        simple,
                        SimpleSelector::Is(_)
                            | SimpleSelector::Where(_)
                            | SimpleSelector::Not(_)
                            | SimpleSelector::Has(_)
                    )
                })
            })
        {
            for observer in &mut self.subjects[previous_subjects..] {
                observer.nonlocal = true;
            }
        }
    }

    fn rules(&mut self, rules: &[Rule], state: &str, nested: bool) {
        for rule in rules {
            match rule {
                Rule::Style(rule) => {
                    for selector in &rule.selectors {
                        self.selector(selector, state, nested);
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

impl StyleResolver {
    /// Invalidates only changed state subtrees when selector dependencies prove
    /// that siblings and ancestors cannot observe the change. Geometry is still
    /// invalidated by the owner; uncertain selectors retain full invalidation.
    /// The locality proof assumes inheritance only from the flat-tree parent,
    /// ancestor-only selector traversal, and global clearing on shared resolver
    /// context changes. Shadow/slot, relational selectors and time-dependent
    /// styles fall back. New cross-element style dependencies must extend these
    /// guards before retaining caches (for example future anchor positioning).
    pub(crate) fn invalidate_user_action_styles(
        &mut self,
        changes: &[(NodeHandle, &'static str)],
    ) -> UserActionStyleInvalidation {
        use UserActionStyleInvalidation::{Full, Local, Unchanged};
        if !self.invalidate_user_action_selectors(changes) {
            return Unchanged;
        }
        let roots = changes
            .iter()
            .filter_map(|(node, state)| {
                self.user_action_dependencies
                    .affects(&self.stylesheets, node, state)
                    .then(|| node.clone())
            })
            .collect::<Vec<_>>();
        let fallback = if self.has_running_transitions() {
            Some("fallback_active_transition")
        } else if self.has_running_animations() {
            Some("fallback_active_animation")
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
        let Some(identities) = invalidation_subtrees(roots) else {
            self.invalidate_style_cache();
            return Full("fallback_shadow_or_slot");
        };
        self.cache
            .retain(|identity, _| !identities.contains(identity));
        self.pseudo_cache
            .retain(|key, _| !identities.contains(&key.0));
        Local
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
                observer.nonlocal
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
