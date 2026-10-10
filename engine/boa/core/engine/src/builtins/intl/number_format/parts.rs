//! Numeric parts are emitted by ICU and CLDR patterns, never inferred from digits.

use crate::{
    Context, JsResult, JsValue, builtins::Array, js_string, object::ObjectInitializer,
    property::Attribute,
};

/// One owned part shared by NumberFormat and native DurationFormat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NumberPart {
    pub(crate) kind: &'static str,
    pub(crate) value: String,
}

pub(crate) fn push_part(parts: &mut Vec<NumberPart>, kind: &'static str, value: &str) {
    if value.is_empty() {
        return;
    }
    if let Some(last) = parts.last_mut().filter(|part| part.kind == kind) {
        last.value.push_str(value);
    } else {
        parts.push(NumberPart {
            kind,
            value: value.to_owned(),
        });
    }
}

/// Bidi controls belong to literal parts, including controls embedded in signs.
pub(crate) fn push_symbol(parts: &mut Vec<NumberPart>, kind: &'static str, value: &str) {
    for character in value.chars() {
        let literal = matches!(character, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
        let mut encoded = [0; 4];
        push_part(
            parts,
            if literal { "literal" } else { kind },
            character.encode_utf8(&mut encoded),
        );
    }
}

#[cfg(test)]
pub(crate) fn parts_to_string(parts: &[NumberPart]) -> String {
    parts.iter().map(|part| part.value.as_str()).collect()
}

#[cfg(test)]
/// Joins owned parts while retaining the leading part's allocation.
pub(crate) fn parts_into_string(parts: Vec<NumberPart>) -> String {
    let mut parts = parts.into_iter();
    let Some(first) = parts.next() else {
        return String::new();
    };
    let additional = parts.as_slice().iter().map(|part| part.value.len()).sum();
    let mut output = first.value;
    output.reserve(additional);
    for part in parts {
        output.push_str(&part.value);
    }
    output
}

#[cfg(test)]
/// Replaces a part value without discarding reusable string capacity.
pub(super) fn replace_value(value: &mut String, replacement: &str) {
    if value.as_str() != replacement {
        value.clear();
        value.push_str(replacement);
    }
}

pub(super) fn parts_to_js(parts: Vec<NumberPart>, context: &mut Context) -> JsResult<JsValue> {
    let result = Array::array_create(0, None, context)?;
    let _result_root = result.clone().root();
    for (index, part) in parts.into_iter().enumerate() {
        let object = ObjectInitializer::new(context)
            .property(js_string!("type"), js_string!(part.kind), Attribute::all())
            .property(
                js_string!("value"),
                js_string!(part.value),
                Attribute::all(),
            )
            .build();
        let _object_root = object.clone().root();
        result.create_data_property_or_throw(index, object, context)?;
    }
    Ok(result.into())
}
