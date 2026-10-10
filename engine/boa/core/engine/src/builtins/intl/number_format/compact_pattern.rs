//! Parsed compact affixes preserve quotes, omitted numerals and sign placement.

use boa_intl_data::{NumberSymbol, NumberSymbols};
use fixed_decimal::Sign;

use super::{
    currency::CurrencyFormatter,
    output::{AffixWriter, NumberOutput},
    pattern::push_label,
};
use crate::{JsNativeError, JsResult};

#[derive(Clone, Debug)]
enum Token {
    Text(String),
    Number,
    Currency,
    Minus,
    Plus,
}

#[derive(Clone, Debug)]
pub(super) struct CompactPattern {
    positive: Vec<Token>,
    negative: Vec<Token>,
    pub(super) zeros: Option<u8>,
    pub(super) sentinel: bool,
}

impl CompactPattern {
    pub(super) fn parse(raw: &str) -> JsResult<Self> {
        let (positive, negative) = split_pattern(raw);
        let (positive, zeros) = tokens(positive)?;
        let negative = if let Some(raw) = negative {
            let (negative, count) = tokens(raw)?;
            if count != zeros {
                return Err(invalid());
            }
            negative
        } else {
            let mut negative = vec![Token::Minus];
            negative.extend(positive.iter().cloned());
            negative
        };
        Ok(Self {
            positive,
            negative,
            zeros,
            sentinel: raw == "0",
        })
    }

    pub(super) fn currency_adjacent_to_letter(
        &self,
        label: &str,
        mut letter: impl FnMut(char) -> bool,
    ) -> bool {
        self.positive.windows(2).any(|tokens| match tokens {
            [Token::Currency, Token::Number] => label.chars().next_back().is_some_and(&mut letter),
            [Token::Number, Token::Currency] => label.chars().next().is_some_and(&mut letter),
            _ => false,
        })
    }

    /// Compact parsing permits at most one numeric slot, including omissions.
    pub(super) fn render<O: NumberOutput>(
        &self,
        number: O,
        sign: Sign,
        symbols: &NumberSymbols<'_>,
        currency: &str,
    ) -> O {
        self.render_impl(number, sign, symbols, currency, None)
    }

    pub(super) fn render_currency<O: NumberOutput>(
        &self,
        number: O,
        sign: Sign,
        symbols: &NumberSymbols<'_>,
        label: &str,
        currency: &CurrencyFormatter,
    ) -> O {
        self.render_impl(number, sign, symbols, label, Some(currency))
    }

    fn render_impl<O: NumberOutput>(
        &self,
        mut number: O,
        sign: Sign,
        symbols: &NumberSymbols<'_>,
        label: &str,
        currency: Option<&CurrencyFormatter>,
    ) -> O {
        let has_minus = self
            .negative
            .iter()
            .any(|token| matches!(token, Token::Minus));
        let tokens = if sign == Sign::Negative || sign == Sign::Positive && has_minus {
            &self.negative
        } else {
            &self.positive
        };
        let mut output = AffixWriter::new(currency);
        if sign == Sign::Positive && !has_minus {
            output.symbol(
                "plusSign",
                symbols.get(NumberSymbol::PlusSign).expect("validated sign"),
            );
        }
        for token in tokens {
            match token {
                Token::Text(text) => push_label(&mut output, "compact", text),
                Token::Number => output.append(std::mem::take(&mut number)),
                Token::Currency => output.symbol("currency", label),
                Token::Minus | Token::Plus => {
                    let plus = sign == Sign::Positive || matches!(token, Token::Plus);
                    let (kind, symbol) = if plus {
                        ("plusSign", NumberSymbol::PlusSign)
                    } else {
                        ("minusSign", NumberSymbol::MinusSign)
                    };
                    output.symbol(kind, symbols.get(symbol).expect("validated sign"));
                }
            }
        }
        output.finish()
    }
}

fn split_pattern(raw: &str) -> (&str, Option<&str>) {
    let mut quoted = false;
    let mut chars = raw.char_indices().peekable();
    while let Some((index, character)) = chars.next() {
        if character == '\'' {
            if chars.peek().is_some_and(|(_, c)| *c == '\'') {
                chars.next();
            } else {
                quoted = !quoted;
            }
        } else if character == ';' && !quoted {
            return (&raw[..index], Some(&raw[index + 1..]));
        }
    }
    (raw, None)
}

fn tokens(raw: &str) -> JsResult<(Vec<Token>, Option<u8>)> {
    let mut chars = raw.chars().peekable();
    let mut quoted = false;
    let mut result = Vec::new();
    let mut zeros = None;
    while let Some(character) = chars.next() {
        if character == '\'' {
            if chars.peek() == Some(&'\'') {
                chars.next();
                text(&mut result, '\'');
            } else {
                quoted = !quoted;
            }
        } else if quoted {
            text(&mut result, character);
        } else {
            match character {
                '0' => {
                    if zeros.is_some() {
                        return Err(invalid());
                    }
                    let mut count = 1_u8;
                    while chars.peek() == Some(&'0') {
                        chars.next();
                        count = count.checked_add(1).ok_or_else(invalid)?;
                    }
                    zeros = Some(count);
                    result.push(Token::Number);
                }
                '¤' => {
                    while chars.peek() == Some(&'¤') {
                        chars.next();
                    }
                    result.push(Token::Currency);
                }
                '-' => result.push(Token::Minus),
                '+' => result.push(Token::Plus),
                '#' | '@' | ';' => return Err(invalid()),
                _ => text(&mut result, character),
            }
        }
    }
    if quoted || result.is_empty() {
        return Err(invalid());
    }
    Ok((result, zeros))
}

fn text(tokens: &mut Vec<Token>, character: char) {
    if let Some(Token::Text(text)) = tokens.last_mut() {
        text.push(character);
    } else {
        tokens.push(Token::Text(character.to_string()));
    }
}

fn invalid() -> crate::JsError {
    JsNativeError::typ()
        .with_message("invalid CLDR compact pattern")
        .into()
}
