//! Owned range templates preserve CLDR order, whitespace and bidi literals.

use super::range::RangePart;
use crate::{JsNativeError, JsResult};

#[derive(Debug)]
enum Token {
    Text(String),
    Endpoint(usize),
}

#[derive(Debug)]
pub(super) struct RangePattern {
    tokens: Vec<Token>,
}

impl RangePattern {
    pub(super) fn parse(raw: &str, endpoints: usize) -> JsResult<Self> {
        if !(1..=2).contains(&endpoints) {
            return Err(invalid());
        }
        let mut tokens = Vec::new();
        let mut seen = [false; 2];
        let mut rest = raw;
        while let Some(index) = rest.find('{') {
            if rest[..index].contains('}') {
                return Err(invalid());
            }
            if index != 0 {
                tokens.push(Token::Text(rest[..index].to_owned()));
            }
            let rest_after = &rest[index..];
            let endpoint = match rest_after.get(..3) {
                Some("{0}") => 0,
                Some("{1}") => 1,
                _ => return Err(invalid()),
            };
            if endpoint >= endpoints || seen[endpoint] {
                return Err(invalid());
            }
            seen[endpoint] = true;
            tokens.push(Token::Endpoint(endpoint));
            rest = &rest_after[3..];
        }
        if rest.contains('}') || !seen[..endpoints].iter().all(|seen| *seen) {
            return Err(invalid());
        }
        if !rest.is_empty() {
            tokens.push(Token::Text(rest.to_owned()));
        }
        Ok(Self { tokens })
    }

    pub(super) fn range(
        &self,
        start: Vec<RangePart>,
        end: Vec<RangePart>,
        space: bool,
    ) -> Vec<RangePart> {
        let mut endpoints = [Some(start), Some(end)];
        let mut result = Vec::new();
        for (index, token) in self.tokens.iter().enumerate() {
            match token {
                Token::Endpoint(endpoint) => {
                    result.extend(endpoints[*endpoint].take().expect("validated endpoint"));
                }
                Token::Text(text) => {
                    let between = index > 0 && index + 1 < self.tokens.len();
                    let mut value = text.clone();
                    if between && space {
                        if !value.starts_with(char::is_whitespace) {
                            value.insert(0, ' ');
                        }
                        if !value.ends_with(char::is_whitespace) {
                            value.push(' ');
                        }
                    }
                    result.push(RangePart::shared("literal", value));
                }
            }
        }
        result
    }
}

fn invalid() -> crate::JsError {
    JsNativeError::typ()
        .with_message("invalid CLDR number range pattern")
        .into()
}
