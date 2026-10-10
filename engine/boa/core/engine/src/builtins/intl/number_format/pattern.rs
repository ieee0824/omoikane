//! CLDR numeric and placeholder patterns retain their original affixes.

use super::{
    currency::CurrencyFormatter,
    output::{AffixWriter, NumberOutput},
};
use crate::{JsNativeError, JsResult};
use boa_intl_data::{NumberSymbol, NumberSymbols};
use fixed_decimal::Sign;

#[derive(Clone, Debug)]
pub(super) enum Affix {
    Literal(String),
    Number,
    Minus,
    Plus,
    Currency,
    Percent,
}

#[derive(Clone, Debug)]
pub(super) struct NumberPattern {
    positive: Vec<Affix>,
    negative: Vec<Affix>,
}

impl NumberPattern {
    pub(super) fn currency_adjacent_to_letter(
        &self,
        label: &str,
        mut is_letter: impl FnMut(char) -> bool,
    ) -> bool {
        self.positive.windows(2).any(|affixes| match affixes {
            [Affix::Currency, Affix::Number] => {
                label.chars().next_back().is_some_and(&mut is_letter)
            }
            [Affix::Number, Affix::Currency] => label.chars().next().is_some_and(&mut is_letter),
            _ => false,
        })
    }

    pub(super) fn parse(pattern: &str) -> JsResult<Self> {
        let mut quoted = false;
        let boundary = pattern.char_indices().find_map(|(index, character)| {
            if character == '\'' {
                quoted = !quoted;
            }
            (character == ';' && !quoted).then_some(index)
        });
        let (positive, negative) = boundary.map_or((pattern, None), |index| {
            (&pattern[..index], Some(&pattern[index + 1..]))
        });
        let positive = parse_affixes(positive)?;
        let negative = negative.map(parse_affixes).transpose()?.unwrap_or_else(|| {
            let mut affixes = vec![Affix::Minus];
            affixes.extend(positive.iter().cloned());
            affixes
        });
        Ok(Self { positive, negative })
    }

    /// Parsed number patterns contain one numeric slot, so its output can move.
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
        if sign == Sign::None && matches!(self.positive.as_slice(), [Affix::Number]) {
            return number;
        }
        let has_minus = self
            .negative
            .iter()
            .any(|affix| matches!(affix, Affix::Minus));
        let affixes = if sign == Sign::Negative || sign == Sign::Positive && has_minus {
            &self.negative
        } else {
            &self.positive
        };
        let mut output = AffixWriter::new(currency);
        if sign == Sign::Positive && !has_minus {
            output.symbol(
                "plusSign",
                symbols
                    .get(NumberSymbol::PlusSign)
                    .expect("validated plus sign"),
            );
        }
        for affix in affixes {
            match affix {
                Affix::Literal(value) => output.push("literal", value),
                Affix::Number => output.append(std::mem::take(&mut number)),
                Affix::Currency => output.symbol("currency", label),
                Affix::Percent => output.symbol(
                    "percentSign",
                    symbols
                        .get(NumberSymbol::PercentSign)
                        .expect("validated percent sign"),
                ),
                Affix::Minus | Affix::Plus => {
                    let plus = sign == Sign::Positive || matches!(affix, Affix::Plus);
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

fn parse_affixes(pattern: &str) -> JsResult<Vec<Affix>> {
    let mut characters = pattern.chars().peekable();
    let mut quoted = false;
    let mut numeric = false;
    let mut affixes = Vec::new();
    while let Some(character) = characters.next() {
        if character == '\'' {
            if characters.peek() == Some(&'\'') {
                characters.next();
                literal(&mut affixes, '\'');
            } else {
                quoted = !quoted;
            }
        } else if quoted {
            literal(&mut affixes, character);
        } else if matches!(character, '#' | '0' | '@') && !numeric {
            numeric = true;
            affixes.push(Affix::Number);
            while characters.peek().is_some_and(|character| {
                matches!(character, '#' | '0'..='9' | '@' | ',' | '.' | 'E' | '+')
            }) {
                characters.next();
            }
        } else {
            match character {
                '-' => affixes.push(Affix::Minus),
                '+' => affixes.push(Affix::Plus),
                '%' => affixes.push(Affix::Percent),
                '¤' => {
                    affixes.push(Affix::Currency);
                    while characters.peek() == Some(&'¤') {
                        characters.next();
                    }
                }
                _ => literal(&mut affixes, character),
            }
        }
    }
    if quoted || !numeric {
        return Err(JsNativeError::typ()
            .with_message("invalid CLDR number pattern")
            .into());
    }
    Ok(affixes)
}

fn literal(affixes: &mut Vec<Affix>, character: char) {
    if let Some(Affix::Literal(value)) = affixes.last_mut() {
        value.push(character);
    } else {
        affixes.push(Affix::Literal(character.to_string()));
    }
}

/// Joins owned numeric output; only repeated numeric placeholders need copies.
pub(super) fn placeholders<O: NumberOutput>(
    pattern: &str,
    mut number: O,
    label: &str,
    kind: &'static str,
) -> O {
    let mut output = AffixWriter::new(None);
    let mut remaining = pattern;
    let mut uses = pattern.matches("{0}").count();
    while let Some(index) = remaining.find('{') {
        push_label(&mut output, kind, &remaining[..index]);
        remaining = &remaining[index..];
        if let Some(rest) = remaining.strip_prefix("{0}") {
            uses -= 1;
            if uses == 0 {
                output.append(std::mem::take(&mut number));
            } else {
                output.append_ref(&number);
            }
            remaining = rest;
        } else if let Some(rest) = remaining.strip_prefix("{1}") {
            output.symbol(kind, label);
            remaining = rest;
        } else {
            push_label(&mut output, kind, "{");
            remaining = &remaining[1..];
        }
    }
    push_label(&mut output, kind, remaining);
    output.finish()
}

pub(super) fn push_label<O: NumberOutput>(
    output: &mut AffixWriter<'_, O>,
    kind: &'static str,
    value: &str,
) {
    let trimmed = value.trim_matches(char::is_whitespace);
    if trimmed.is_empty() {
        output.push("literal", value);
        return;
    }
    let start = value.len() - value.trim_start_matches(char::is_whitespace).len();
    output.push("literal", &value[..start]);
    output.symbol(kind, trimmed);
    output.push("literal", &value[start + trimmed.len()..]);
}
