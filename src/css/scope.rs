//! Parsing for CSS Scoping `@scope` rule preludes.

use super::tokenizer::render_tokens;
use super::{CssToken, Selector, SimpleSelector, parse_selector_list, tokenize};

/// Parsed start and end boundaries for an `@scope` rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScopePrelude {
    /// An omitted start boundary means the document root (or the enclosing scope root).
    pub(crate) start: Option<Vec<Selector>>,
    pub(crate) end: Option<Vec<Selector>>,
}

/// Parses `[(<scope-start>)]? [to (<scope-end>)]?`.
///
/// Boundary selector lists are strict: an invalid selector invalidates the
/// complete scope rule instead of silently dropping a branch.
pub(crate) fn parse_scope_prelude(input: &str) -> Option<ScopePrelude> {
    let tokens = tokenize(input).ok()?;
    let mut cursor = TokenCursor::new(&tokens);
    cursor.skip_whitespace();

    let start = if cursor.peek() == Some(&CssToken::ParenOpen) {
        Some(parse_boundary(&mut cursor)?)
    } else {
        None
    };

    cursor.skip_whitespace();
    let end = match cursor.peek() {
        Some(CssToken::Ident(keyword)) if keyword.eq_ignore_ascii_case("to") => {
            cursor.next();
            cursor.skip_whitespace();
            Some(parse_boundary(&mut cursor)?)
        }
        _ => None,
    };
    cursor.skip_whitespace();

    if cursor.peek().is_some() || start.is_none() && end.is_none() && !input.trim().is_empty() {
        return None;
    }
    Some(ScopePrelude { start, end })
}

fn parse_boundary(cursor: &mut TokenCursor<'_>) -> Option<Vec<Selector>> {
    if cursor.next() != Some(&CssToken::ParenOpen) {
        return None;
    }
    let start = cursor.index;
    let mut depth = 1usize;
    while let Some(token) = cursor.next() {
        match token {
            CssToken::ParenOpen => depth += 1,
            CssToken::ParenClose => {
                depth -= 1;
                if depth == 0 {
                    let selectors = parse_boundary_selectors(
                        &cursor.tokens[start..cursor.index - 1],
                    )?;
                    if selectors.iter().any(selector_has_pseudo_element) {
                        return None;
                    }
                    return Some(selectors);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_boundary_selectors(tokens: &[CssToken]) -> Option<Vec<Selector>> {
    let mut selectors = Vec::new();
    let mut branch_start = 0usize;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        match token {
            CssToken::ParenOpen => paren_depth += 1,
            CssToken::ParenClose => paren_depth = paren_depth.saturating_sub(1),
            CssToken::BracketOpen => bracket_depth += 1,
            CssToken::BracketClose => bracket_depth = bracket_depth.saturating_sub(1),
            CssToken::Comma if paren_depth == 0 && bracket_depth == 0 => {
                selectors.push(parse_boundary_branch(&tokens[branch_start..index])?);
                branch_start = index + 1;
            }
            _ => {}
        }
    }
    selectors.push(parse_boundary_branch(&tokens[branch_start..])?);
    Some(selectors)
}

fn parse_boundary_branch(tokens: &[CssToken]) -> Option<Selector> {
    let first = tokens.iter().find(|token| **token != CssToken::Whitespace)?;
    let mut text = render_tokens(tokens).trim().to_string();
    if matches!(first, CssToken::Delim('>' | '+' | '~')) {
        text.insert_str(0, ":scope ");
    }
    let mut selectors = parse_selector_list(&text).ok()?;
    (selectors.len() == 1).then(|| selectors.remove(0))
}

fn selector_has_pseudo_element(selector: &Selector) -> bool {
    selector.parts.iter().any(|part| {
        part.simples.iter().any(|simple| match simple {
            SimpleSelector::PseudoElement(_) => true,
            SimpleSelector::PseudoClass(name)
                if matches!(name.to_ascii_lowercase().as_str(), "before" | "after") => true,
            SimpleSelector::Is(selectors)
            | SimpleSelector::Where(selectors)
            | SimpleSelector::Not(selectors) => selectors.iter().any(selector_has_pseudo_element),
            SimpleSelector::Has(relative) => relative
                .iter()
                .any(|relative| selector_has_pseudo_element(&relative.selector)),
            _ => false,
        })
    })
}

struct TokenCursor<'a> {
    tokens: &'a [CssToken],
    index: usize,
}

impl<'a> TokenCursor<'a> {
    fn new(tokens: &'a [CssToken]) -> Self {
        Self { tokens, index: 0 }
    }

    fn peek(&self) -> Option<&'a CssToken> {
        self.tokens.get(self.index)
    }

    fn next(&mut self) -> Option<&'a CssToken> {
        let token = self.tokens.get(self.index)?;
        self.index += 1;
        Some(token)
    }

    fn skip_whitespace(&mut self) {
        while self.peek() == Some(&CssToken::Whitespace) {
            self.index += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scope_boundaries() {
        let parsed = parse_scope_prelude("(.article, #featured) to (.ad, :scope > footer)")
            .expect("valid @scope prelude");
        assert_eq!(parsed.start.as_ref().map(Vec::len), Some(2));
        assert_eq!(parsed.end.as_ref().map(Vec::len), Some(2));
        assert!(parse_scope_prelude("").is_some());
        assert!(parse_scope_prelude("to (.limit)").is_some());
        assert!(parse_scope_prelude("(.article) to (> .limit, + aside)").is_some());
    }

    #[test]
    fn rejects_malformed_scope_boundaries() {
        for invalid in [
            ".article",
            "(.article) trailing",
            "(.article) to",
            "(.article,)",
            "(.article) to (::before)",
            "(:is(.article, ::before))",
            "()",
        ] {
            assert!(parse_scope_prelude(invalid).is_none(), "accepted {invalid:?}");
        }
    }
}
