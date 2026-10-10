//! Shared semantic writers for scalar text and typed numeric parts.

use std::fmt;

use writeable::{Part, PartsWrite};

use super::{
    currency::CurrencyFormatter,
    parts::{NumberPart, push_part, push_symbol as push_part_symbol},
};

/// Only the semantic edge matters when adjoining a currency and a number.
#[derive(Clone, Copy, Debug)]
pub(super) struct Edge {
    pub(super) kind: &'static str,
    pub(super) character: char,
}

/// The same numeric and CLDR decisions can emit text or preserve part boundaries.
pub(super) trait NumberOutput: Default + Clone {
    fn reserve_text(&mut self, bytes: usize);
    fn push(&mut self, kind: &'static str, value: &str);
    fn symbol(&mut self, kind: &'static str, value: &str);
    fn append(&mut self, other: Self);
    fn append_ref(&mut self, other: &Self);
    fn first_edge(&self) -> Option<Edge>;
    fn last_edge(&self) -> Option<Edge>;
    fn rename(&mut self, from: &'static str, to: &'static str);
    fn merge_unit_labels(&mut self);
}

impl NumberOutput for Vec<NumberPart> {
    fn reserve_text(&mut self, _: usize) {}

    fn push(&mut self, kind: &'static str, value: &str) {
        push_part(self, kind, value);
    }

    fn symbol(&mut self, kind: &'static str, value: &str) {
        push_part_symbol(self, kind, value);
    }

    fn append(&mut self, mut other: Self) {
        Vec::append(self, &mut other);
    }

    fn append_ref(&mut self, other: &Self) {
        self.extend_from_slice(other);
    }

    fn first_edge(&self) -> Option<Edge> {
        self.iter().find_map(|part| {
            part.value.chars().next().map(|character| Edge {
                kind: part.kind,
                character,
            })
        })
    }

    fn last_edge(&self) -> Option<Edge> {
        self.iter().rev().find_map(|part| {
            part.value.chars().next_back().map(|character| Edge {
                kind: part.kind,
                character,
            })
        })
    }

    fn rename(&mut self, from: &'static str, to: &'static str) {
        for part in self {
            if part.kind == from {
                part.kind = to;
            }
        }
    }

    fn merge_unit_labels(&mut self) {
        let mut index = 0;
        while index + 2 < self.len() {
            if self[index].kind == "unit"
                && self[index + 1].kind == "literal"
                && self[index + 1].value.chars().all(char::is_whitespace)
                && self[index + 2].kind == "unit"
            {
                let separator = self.remove(index + 1);
                let next = self.remove(index + 1);
                self[index].value.push_str(&separator.value);
                self[index].value.push_str(&next.value);
            } else {
                index += 1;
            }
        }
    }
}

/// Scalar formatting owns one buffer and retains no individual part strings.
#[derive(Clone, Debug, Default)]
pub(super) struct NumberText {
    value: String,
    first: Option<Edge>,
    last: Option<Edge>,
}

impl NumberText {
    pub(super) fn into_string(self) -> String {
        self.value
    }
}

impl NumberOutput for NumberText {
    fn reserve_text(&mut self, bytes: usize) {
        self.value.reserve(bytes);
    }

    fn push(&mut self, kind: &'static str, value: &str) {
        let Some(character) = value.chars().next() else {
            return;
        };
        self.first.get_or_insert(Edge { kind, character });
        self.last = value
            .chars()
            .next_back()
            .map(|character| Edge { kind, character });
        self.value.push_str(value);
    }

    fn symbol(&mut self, kind: &'static str, value: &str) {
        let was_empty = self.value.is_empty();
        self.push(kind, value);
        if value.is_empty() {
            return;
        }
        if was_empty && let Some(first) = &mut self.first {
            first.kind = symbol_kind(kind, first.character);
        }
        if let Some(last) = &mut self.last {
            last.kind = symbol_kind(kind, last.character);
        }
    }

    fn append(&mut self, mut other: Self) {
        if other.value.is_empty() {
            return;
        }
        if self.value.is_empty() {
            *self = other;
            return;
        }
        self.last = other.last;
        // A numeric buffer usually dwarfs its prefix. Retain that allocation
        // when a CLDR prefix is prepended instead of copying all its digits.
        if other.value.capacity() > self.value.capacity() {
            other.value.insert_str(0, &self.value);
            self.value = other.value;
        } else {
            self.value.push_str(&other.value);
        }
    }

    fn append_ref(&mut self, other: &Self) {
        if other.value.is_empty() {
            return;
        }
        self.first = self.first.or(other.first);
        self.last = other.last;
        self.value.push_str(&other.value);
    }

    fn first_edge(&self) -> Option<Edge> {
        self.first
    }

    fn last_edge(&self) -> Option<Edge> {
        self.last
    }

    fn rename(&mut self, from: &'static str, to: &'static str) {
        for edge in [&mut self.first, &mut self.last].into_iter().flatten() {
            if edge.kind == from {
                edge.kind = to;
            }
        }
    }

    fn merge_unit_labels(&mut self) {}
}

/// Currency borrowing is confined to one pattern render, never formatter state.
pub(super) struct AffixWriter<'a, O> {
    output: O,
    currency: Option<&'a CurrencyFormatter>,
}

impl<'a, O: NumberOutput> AffixWriter<'a, O> {
    pub(super) fn new(currency: Option<&'a CurrencyFormatter>) -> Self {
        Self {
            output: O::default(),
            currency,
        }
    }

    pub(super) fn finish(self) -> O {
        self.output
    }

    pub(super) fn push(&mut self, kind: &'static str, value: &str) {
        let edge = value
            .chars()
            .next()
            .map(|character| Edge { kind, character });
        self.space_before(edge);
        self.output.push(kind, value);
    }

    pub(super) fn symbol(&mut self, kind: &'static str, value: &str) {
        let edge = value.chars().next().map(|character| Edge {
            kind: symbol_kind(kind, character),
            character,
        });
        self.space_before(edge);
        self.output.symbol(kind, value);
    }

    pub(super) fn append(&mut self, value: O) {
        self.space_before(value.first_edge());
        self.output.append(value);
    }

    pub(super) fn append_ref(&mut self, value: &O) {
        self.space_before(value.first_edge());
        self.output.append_ref(value);
    }

    fn space_before(&mut self, right: Option<Edge>) {
        if let Some((currency, (left, right))) =
            self.currency.zip(self.output.last_edge().zip(right))
            && let Some(spacing) = currency.spacing_between(left, right)
        {
            self.output.push("literal", spacing);
        }
    }
}

pub(super) fn symbol_kind(kind: &'static str, character: char) -> &'static str {
    if matches!(character, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
    {
        "literal"
    } else {
        kind
    }
}

pub(super) fn push_symbol<O: NumberOutput>(output: &mut O, kind: &'static str, value: &str) {
    output.symbol(kind, value);
}

/// An ICU write view borrows separator overrides only for the current write.
pub(super) struct DecimalWriter<'a, O> {
    pub(super) output: O,
    active: Option<&'static str>,
    decimal: Option<&'a str>,
    group: Option<&'a str>,
    exponent: bool,
}

impl<'a, O: NumberOutput> DecimalWriter<'a, O> {
    pub(super) fn new(decimal: Option<&'a str>, group: Option<&'a str>, exponent: bool) -> Self {
        Self {
            output: O::default(),
            active: None,
            decimal,
            group,
            exponent,
        }
    }
}

impl<O: NumberOutput> fmt::Write for DecimalWriter<'_, O> {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let kind = match self.active {
            Some("integer") if self.exponent => "exponentInteger",
            Some(kind) => kind,
            None => "literal",
        };
        push_symbol(&mut self.output, kind, value);
        Ok(())
    }
}

impl<O: NumberOutput> PartsWrite for DecimalWriter<'_, O> {
    type SubPartsWrite = Self;

    fn with_part(
        &mut self,
        part: Part,
        mut write: impl FnMut(&mut Self) -> fmt::Result,
    ) -> fmt::Result {
        let replacement = match part.value {
            "decimal" => self.decimal,
            "group" => self.group,
            _ => None,
        };
        if let Some(value) = replacement {
            self.output.push(part.value, value);
            return Ok(());
        }
        let previous = self.active.replace(part.value);
        let result = write(self);
        self.active = previous;
        result
    }
}
