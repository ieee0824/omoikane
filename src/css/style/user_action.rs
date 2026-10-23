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
    subjects: Vec<Selector>,
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
        self.subjects.push(Selector {
            parts: vec![SelectorPart {
                combinator: None,
                simples,
            }],
        });
    }

    fn selector(&mut self, selector: &Selector, state: &str, nested: bool) {
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
                .any(|selector| super::super::matcher::matches_selector(node, selector))
    }
}

impl StyleResolver {
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
