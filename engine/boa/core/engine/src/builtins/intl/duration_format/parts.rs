//! Owned duration parts, typed ICU list-element replacement, and JS publication.

use super::options::DurationUnit;
use crate::{
    Context, JsNativeError, JsResult,
    builtins::{
        OrdinaryObject,
        array::Array,
        intl::number_format::{MathematicalValue, NativeNumberFormatter, NumericFormatOptions},
    },
    js_string,
    object::JsObject,
};
use icu_list::ListFormatter;
use std::fmt::{self, Write};
use writeable::{LengthHint, Part, PartsWrite, Writeable};

/// One native duration part; unitless literals belong to the clock or list.
#[derive(Debug, PartialEq)]
pub(super) struct DurationPart {
    pub(super) kind: &'static str,
    pub(super) value: String,
    pub(super) unit: Option<DurationUnit>,
}

impl DurationPart {
    pub(super) fn literal(value: &str) -> Self {
        Self {
            kind: "literal",
            value: value.to_owned(),
            unit: None,
        }
    }
}

#[cfg(test)]
pub(super) fn join(parts: &[DurationPart]) -> String {
    parts.iter().map(|part| part.value.as_str()).collect()
}

/// Group selection is shared while scalar output avoids both kinds of parts.
pub(super) trait DurationOutput: Default {
    fn is_empty(&self) -> bool;
    fn literal(&mut self, value: &str);
    fn append(&mut self, other: Self);
    fn number(
        formatter: &NativeNumberFormatter,
        value: MathematicalValue,
        digits: &NumericFormatOptions,
        unit: DurationUnit,
    ) -> Self;
}

impl DurationOutput for Vec<DurationPart> {
    fn is_empty(&self) -> bool {
        Vec::is_empty(self)
    }

    fn literal(&mut self, value: &str) {
        self.push(DurationPart::literal(value));
    }

    fn append(&mut self, other: Self) {
        self.extend(other);
    }

    fn number(
        formatter: &NativeNumberFormatter,
        value: MathematicalValue,
        digits: &NumericFormatOptions,
        unit: DurationUnit,
    ) -> Self {
        formatter
            .format(value, digits)
            .into_iter()
            .map(|part| DurationPart {
                kind: part.kind,
                value: part.value,
                unit: Some(unit),
            })
            .collect()
    }
}

impl DurationOutput for String {
    fn is_empty(&self) -> bool {
        String::is_empty(self)
    }

    fn literal(&mut self, value: &str) {
        self.push_str(value);
    }

    fn append(&mut self, other: Self) {
        if self.is_empty() {
            *self = other;
        } else {
            self.push_str(&other);
        }
    }

    fn number(
        formatter: &NativeNumberFormatter,
        value: MathematicalValue,
        digits: &NumericFormatOptions,
        _: DurationUnit,
    ) -> Self {
        formatter.format_text(value, digits)
    }
}

/// A short-lived, zero-copy view of one duration list element.
#[derive(Clone, Copy)]
struct DurationGroup<'a>(&'a [DurationPart]);

impl Writeable for DurationGroup<'_> {
    fn write_to<W: Write + ?Sized>(&self, sink: &mut W) -> fmt::Result {
        for part in self.0 {
            sink.write_str(&part.value)?;
        }
        Ok(())
    }

    fn writeable_length_hint(&self) -> LengthHint {
        LengthHint::exact(self.0.iter().map(|part| part.value.len()).sum())
    }
}

enum ListPiece {
    Literal(String),
    Element,
}

/// Records list literals and element positions without copying element text.
struct ListPartsCollector {
    pieces: Vec<ListPiece>,
    inside_element: bool,
}

impl Write for ListPartsCollector {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        if self.inside_element || value.is_empty() {
            return Ok(());
        }
        if let Some(ListPiece::Literal(last)) = self.pieces.last_mut() {
            last.push_str(value);
        } else {
            self.pieces.push(ListPiece::Literal(value.to_owned()));
        }
        Ok(())
    }
}

impl PartsWrite for ListPartsCollector {
    type SubPartsWrite = Self;

    fn with_part(
        &mut self,
        part: Part,
        mut write: impl FnMut(&mut Self::SubPartsWrite) -> fmt::Result,
    ) -> fmt::Result {
        if part != icu_list::parts::ELEMENT || self.inside_element {
            return write(self);
        }
        self.inside_element = true;
        let result = write(self);
        self.inside_element = false;
        result?;
        self.pieces.push(ListPiece::Element);
        Ok(())
    }
}

fn groups<'a>(groups: &'a [Vec<DurationPart>]) -> impl Iterator<Item = DurationGroup<'a>> + Clone {
    groups.iter().map(|parts| DurationGroup(parts))
}

/// Reference list text used to verify that scalar output matches typed groups.
#[cfg(test)]
pub(super) fn format_list(formatter: &ListFormatter, groups: &[Vec<DurationPart>]) -> String {
    formatter
        .format(self::groups(groups))
        .write_to_string()
        .into_owned()
}

/// Reuses a sole group's buffer; only multiple groups need list concatenation.
pub(super) fn format_text_list(formatter: &ListFormatter, mut groups: Vec<String>) -> String {
    if groups.len() <= 1 {
        return groups.pop().unwrap_or_default();
    }
    formatter
        .format(groups.iter().map(String::as_str))
        .write_to_string()
        .into_owned()
}

fn push_literal(parts: &mut Vec<DurationPart>, value: String) {
    if value.is_empty() {
        return;
    }
    if let Some(last) = parts.last_mut()
        && last.kind == "literal"
        && last.unit.is_none()
    {
        last.value.push_str(&value);
    } else {
        parts.push(DurationPart {
            kind: "literal",
            value,
            unit: None,
        });
    }
}

pub(super) fn list_parts(
    formatter: &ListFormatter,
    groups: Vec<Vec<DurationPart>>,
) -> JsResult<Vec<DurationPart>> {
    let mut collector = ListPartsCollector {
        pieces: Vec::new(),
        inside_element: false,
    };
    formatter
        .format(self::groups(&groups))
        .write_to_parts(&mut collector)
        .map_err(|error| {
            JsNativeError::typ().with_message(format!("duration list formatting: {error}"))
        })?;
    let mut groups = groups.into_iter();
    let mut parts = Vec::new();
    for piece in collector.pieces {
        match piece {
            ListPiece::Literal(value) => push_literal(&mut parts, value),
            ListPiece::Element => parts.extend(groups.next().ok_or_else(|| {
                JsNativeError::typ().with_message("duration list element data is inconsistent")
            })?),
        }
    }
    if groups.next().is_some() {
        return Err(JsNativeError::typ()
            .with_message("duration list element data is inconsistent")
            .into());
    }
    Ok(parts)
}

pub(super) fn to_array(parts: Vec<DurationPart>, context: &mut Context) -> JsResult<JsObject> {
    let result = Array::array_create(0, None, context)?;
    let _result_root = result.clone().root();
    for (index, part) in parts.into_iter().enumerate() {
        let object = context
            .intrinsics()
            .templates()
            .ordinary_object()
            .create(OrdinaryObject, vec![]);
        let _object_root = object.clone().root();
        object.create_data_property_or_throw(js_string!("type"), js_string!(part.kind), context)?;
        object.create_data_property_or_throw(
            js_string!("value"),
            js_string!(part.value),
            context,
        )?;
        if let Some(unit) = part.unit {
            object.create_data_property_or_throw(
                js_string!("unit"),
                js_string!(unit.singular()),
                context,
            )?;
        }
        result.create_data_property_or_throw(index, object, context)?;
    }
    Ok(result)
}
