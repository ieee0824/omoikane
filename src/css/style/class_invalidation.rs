//! Conservative computed-style cache reach for ordinary HTML class changes.
use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ClassReach {
    #[default]
    Absent,
    Subtree,
    NonLocal,
}

fn tokens_observe_class(source: &str, class: &str) -> bool {
    if selector_tokens_observe_attribute(source, "class") != AttributeTokenObservation::Absent {
        return true;
    }
    let Ok(tokens) = super::super::tokenizer::tokenize(source) else {
        return true;
    };
    let tokens: Vec<_> = tokens
        .iter()
        .filter(|t| **t != CssToken::Whitespace)
        .collect();
    tokens.windows(2).any(|pair| {
        matches!(pair, [CssToken::Delim('.'), CssToken::Ident(name)]
            if !class.is_empty() && name.eq_ignore_ascii_case(class))
    })
}

fn simple_reach(simple: &SimpleSelector, class: &str) -> ClassReach {
    match simple {
        SimpleSelector::Class(name) if !class.is_empty() && name.eq_ignore_ascii_case(class) => {
            ClassReach::Subtree
        }
        SimpleSelector::Attribute { name, .. }
            if name
                .rsplit('|')
                .next()
                .is_some_and(|n| n.eq_ignore_ascii_case("class")) =>
        {
            ClassReach::Subtree
        }
        SimpleSelector::Is(selectors)
        | SimpleSelector::Where(selectors)
        | SimpleSelector::Not(selectors) => selectors
            .iter()
            .map(|selector| {
                let reach = selector_reach(selector, class);
                if reach != ClassReach::Absent && selector.parts.len() != 1 {
                    ClassReach::NonLocal
                } else {
                    reach
                }
            })
            .max()
            .unwrap_or_default(),
        SimpleSelector::Has(selectors) => {
            if selectors
                .iter()
                .any(|relative| selector_reach(&relative.selector, class) != ClassReach::Absent)
            {
                ClassReach::NonLocal
            } else {
                ClassReach::Absent
            }
        }
        SimpleSelector::PseudoClass(name) | SimpleSelector::PseudoElement(name)
            if name.contains('(') && tokens_observe_class(name, class) =>
        {
            ClassReach::NonLocal
        }
        _ => ClassReach::Absent,
    }
}

fn selector_reach(selector: &Selector, class: &str) -> ClassReach {
    let mut reach = ClassReach::Absent;
    for (index, part) in selector.parts.iter().enumerate() {
        let current = part
            .simples
            .iter()
            .map(|simple| simple_reach(simple, class))
            .max()
            .unwrap_or_default();
        // Once a descendant/child edge is taken, later sibling edges stay
        // within this subtree. An immediate sibling edge can leave it.
        let escapes = selector.parts.get(index + 1).is_some_and(|part| {
            !matches!(
                part.combinator,
                Some(Combinator::Descendant | Combinator::Child)
            )
        });
        reach = reach.max(if current != ClassReach::Absent && escapes {
            ClassReach::NonLocal
        } else {
            current
        });
    }
    if reach != ClassReach::Absent && selector_contains_shadow_pseudo(selector) {
        ClassReach::NonLocal
    } else {
        reach
    }
}

fn rules_reach(rules: &[Rule], class: &str, nested_style: bool) -> ClassReach {
    rules
        .iter()
        .map(|rule| match rule {
            Rule::Style(rule) => {
                let selectors = rule
                    .selectors
                    .iter()
                    .map(|selector| selector_reach(selector, class))
                    .max()
                    .unwrap_or_default();
                let nested = rules_reach(&rule.rules, class, true);
                let reach = selectors.max(nested);
                if (nested_style && reach != ClassReach::Absent)
                    || (selectors != ClassReach::Absent && !rule.rules.is_empty())
                {
                    ClassReach::NonLocal
                } else {
                    reach
                }
            }
            Rule::At(rule) => {
                if rule.name.eq_ignore_ascii_case("scope")
                    && tokens_observe_class(&rule.prelude, class)
                {
                    ClassReach::NonLocal
                } else {
                    rule.block
                        .as_deref()
                        .map(|rules| rules_reach(rules, class, nested_style))
                        .unwrap_or_default()
                }
            }
            Rule::FontFace(_) => ClassReach::Absent,
        })
        .max()
        .unwrap_or_default()
}

impl StyleResolver {
    fn class_reach(&mut self, class: &str) -> ClassReach {
        if let Some(reach) = self.class_selector_dependencies.get(class) {
            return *reach;
        }
        let reach = self
            .stylesheets
            .iter()
            .map(|input| rules_reach(&input.stylesheet.rules, class, false))
            .max()
            .unwrap_or_default();
        // Dynamic class names must not cause unbounded resolver cache growth.
        if self.class_selector_dependencies.len() < 1024 {
            self.class_selector_dependencies
                .insert(class.to_owned(), reach);
        }
        reach
    }

    /// Discards styles reachable from an ordinary HTML class change. Uncertain
    /// selector reach, document roots, and shadow/slot boundaries clear all
    /// computed styles. Geometry must still be invalidated by the caller.
    pub(crate) fn invalidate_class_subtree(
        &mut self,
        node: &NodeHandle,
        previous: Option<&str>,
        current: Option<&str>,
        non_scalar: bool,
    ) -> &'static str {
        self.selector_match_cache = SelectorMatchCache::default();
        let old: HashSet<_> = previous
            .unwrap_or_default()
            .split_ascii_whitespace()
            .collect();
        let new: HashSet<_> = current
            .unwrap_or_default()
            .split_ascii_whitespace()
            .collect();
        let is_root = node
            .parent_node()
            .is_some_and(|p| p.node_type() == NodeType::Document);
        let fallback = if !node.is_html_element() || node.prefix().is_some() || is_root {
            Some("fallback_class_root_or_namespace")
        } else if non_scalar {
            Some("fallback_class_non_scalar")
        } else if self.class_reach("") == ClassReach::NonLocal
            || old
                .symmetric_difference(&new)
                .any(|class| self.class_reach(class) == ClassReach::NonLocal)
        {
            Some("fallback_class_selector")
        } else {
            None
        };
        let identities = if fallback.is_none() {
            inline_invalidation_subtree(node)
        } else {
            None
        };
        let Some(identities) = identities else {
            self.cache.clear();
            self.pseudo_cache.clear();
            return fallback.unwrap_or("fallback_class_shadow_or_slot");
        };
        self.cache
            .retain(|identity, _| !identities.contains(identity));
        self.pseudo_cache
            .retain(|key, _| !identities.contains(&key.0));
        "local_class_subtree"
    }
}

#[cfg(test)]
mod tests;
