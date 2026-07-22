//! CSS Conditional Rules `@supports` condition parsing and evaluation.

use super::tokenizer::render_tokens;
use super::{CssToken, parse_selector_list, supports_declaration, tokenize};

/// Evaluates a CSS supports condition using the same declaration and selector
/// parsers as the cascade and DOM APIs.
pub(crate) fn supports_condition_matches(input: &str) -> bool {
    let Ok(tokens) = tokenize(input) else {
        return false;
    };
    let tokens = trim_tokens(&tokens);
    if tokens.is_empty() {
        return false;
    }

    // CSS.supports() also accepts an unwrapped declaration in its one-argument
    // form. This does not affect @supports, whose grammar requires parentheses.
    if let Some(result) = evaluate_declaration(tokens) {
        return result;
    }

    SupportsConditionParser::new(tokens.to_vec())
        .parse_complete()
        .unwrap_or(false)
}

struct SupportsConditionParser {
    tokens: Vec<CssToken>,
    index: usize,
}

impl SupportsConditionParser {
    fn new(tokens: Vec<CssToken>) -> Self {
        Self { tokens, index: 0 }
    }

    fn parse_complete(mut self) -> Option<bool> {
        self.skip_whitespace();
        let result = self.parse_condition()?;
        self.skip_whitespace();
        (self.index == self.tokens.len()).then_some(result)
    }

    fn parse_condition(&mut self) -> Option<bool> {
        if self.peek_ident("not") {
            self.index += 1;
            if !self.consume_whitespace() {
                return None;
            }
            return Some(!self.parse_in_parens()?);
        }

        let mut result = self.parse_in_parens()?;
        let mut operator = None;
        loop {
            let separated = self.consume_whitespace();
            let next_operator = if self.peek_ident("and") {
                true
            } else if self.peek_ident("or") {
                false
            } else {
                return Some(result);
            };
            if !separated || operator.is_some_and(|current| current != next_operator) {
                return None;
            }
            operator = Some(next_operator);
            self.index += 1;
            if !self.consume_whitespace() {
                return None;
            }
            let next = self.parse_in_parens()?;
            result = if next_operator {
                result && next
            } else {
                result || next
            };
        }
    }

    fn parse_in_parens(&mut self) -> Option<bool> {
        self.skip_whitespace();
        if matches!(self.tokens.get(self.index), Some(CssToken::ParenOpen)) {
            let inner = self.take_parenthesized()?;
            if let Some(result) = evaluate_declaration(&inner) {
                return Some(result);
            }
            return Some(
                SupportsConditionParser::new(inner)
                    .parse_complete()
                    .unwrap_or(false),
            );
        }

        let Some(CssToken::Ident(function)) = self.tokens.get(self.index) else {
            return None;
        };
        let function = function.clone();
        if !matches!(self.tokens.get(self.index + 1), Some(CssToken::ParenOpen)) {
            return None;
        }
        self.index += 1;
        let argument = self.take_parenthesized()?;
        if function.eq_ignore_ascii_case("selector") {
            let selector = render_tokens(trim_tokens(&argument));
            return Some(parse_selector_list(&selector).is_ok());
        }
        // Unknown functions are valid general-enclosed conditions and evaluate
        // false for forward compatibility.
        Some(false)
    }

    fn take_parenthesized(&mut self) -> Option<Vec<CssToken>> {
        if !matches!(self.tokens.get(self.index), Some(CssToken::ParenOpen)) {
            return None;
        }
        self.index += 1;
        let start = self.index;
        let mut depth = 0usize;
        while let Some(token) = self.tokens.get(self.index) {
            match token {
                CssToken::ParenOpen => depth += 1,
                CssToken::ParenClose if depth == 0 => {
                    let inner = self.tokens[start..self.index].to_vec();
                    self.index += 1;
                    return Some(inner);
                }
                CssToken::ParenClose => depth -= 1,
                _ => {}
            }
            self.index += 1;
        }
        None
    }

    fn peek_ident(&self, expected: &str) -> bool {
        matches!(self.tokens.get(self.index), Some(CssToken::Ident(value)) if value.eq_ignore_ascii_case(expected))
    }

    fn skip_whitespace(&mut self) {
        self.consume_whitespace();
    }

    fn consume_whitespace(&mut self) -> bool {
        let start = self.index;
        while matches!(self.tokens.get(self.index), Some(CssToken::Whitespace)) {
            self.index += 1;
        }
        self.index != start
    }
}

fn evaluate_declaration(tokens: &[CssToken]) -> Option<bool> {
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut colon = None;
    for (index, token) in tokens.iter().enumerate() {
        match token {
            CssToken::ParenOpen => paren_depth += 1,
            CssToken::ParenClose => paren_depth = paren_depth.saturating_sub(1),
            CssToken::BracketOpen => bracket_depth += 1,
            CssToken::BracketClose => bracket_depth = bracket_depth.saturating_sub(1),
            CssToken::Colon if paren_depth == 0 && bracket_depth == 0 => {
                if colon.is_some() {
                    return None;
                }
                colon = Some(index);
            }
            _ => {}
        }
    }
    let colon = colon?;
    let property = render_tokens(trim_tokens(&tokens[..colon]));
    let value = render_tokens(trim_tokens(&tokens[colon + 1..]));
    Some(supports_declaration(&property, &value))
}

fn trim_tokens(tokens: &[CssToken]) -> &[CssToken] {
    let start = tokens
        .iter()
        .position(|token| !matches!(token, CssToken::Whitespace))
        .unwrap_or(tokens.len());
    let end = tokens
        .iter()
        .rposition(|token| !matches!(token, CssToken::Whitespace))
        .map_or(start, |index| index + 1);
    &tokens[start..end]
}

#[cfg(test)]
mod tests {
    use super::supports_condition_matches;

    #[test]
    fn evaluates_declarations_and_boolean_conditions() {
        assert!(supports_condition_matches("(display: grid)"));
        assert!(supports_condition_matches("color: red"));
        assert!(supports_condition_matches(
            "color: something-pointless var(--theme)"
        ));
        assert!(supports_condition_matches(
            "color: something-pointless(var(--theme))"
        ));
        assert!(!supports_condition_matches("width: blah"));
        assert!(supports_condition_matches("width: calc(100% - 24px)"));
        assert!(supports_condition_matches("width: calc(-1px)"));
        assert!(!supports_condition_matches("width: calc(-1)"));
        assert!(!supports_condition_matches("(unknown-property: value)"));
        assert!(supports_condition_matches(
            "(display: grid) and (color: red)"
        ));
        assert!(supports_condition_matches(
            "(unknown-property: value) or (display: block)"
        ));
        assert!(supports_condition_matches("not (unknown-property: value)"));
        assert!(!supports_condition_matches("not (display: block)"));
    }

    #[test]
    fn evaluates_nested_general_enclosed_and_selector_conditions() {
        assert!(supports_condition_matches(
            "((display: grid) and (color: red))"
        ));
        assert!(!supports_condition_matches("future-feature(example)"));
        assert!(supports_condition_matches("not future-feature(example)"));
        assert!(supports_condition_matches("selector(main > :has(span))"));
        assert!(!supports_condition_matches("selector(main >)"));
    }

    #[test]
    fn rejects_mixed_or_unseparated_operators() {
        assert!(!supports_condition_matches(
            "(display: block) and (color: red) or (width: 1px)"
        ));
        assert!(!supports_condition_matches("not(display: block)"));
        assert!(!supports_condition_matches(
            "(display: block)and(color: red)"
        ));
    }
}
