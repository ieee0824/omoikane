//! Grammar and computed-value conversion for CSS `zoom`.

use super::{
    ComputedValue, DeclarationValidation, LengthMathValue, ResolutionContext, Value,
    evaluate_length_math_value, is_css_wide_keyword,
};

fn numeric_zoom(value: &Value) -> Option<f32> {
    let value = match evaluate_length_math_value(value, ResolutionContext::default())? {
        LengthMathValue::Number(value) => value,
        LengthMathValue::Length(value) => value.resolve(1.0),
    };
    (value.is_finite() && value >= 0.0).then_some(if value == 0.0 { 1.0 } else { value })
}

pub(super) fn validate(name: &str, value: &Value) -> Option<DeclarationValidation> {
    if name != "zoom" {
        return None;
    }
    let computed = match value {
        Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("normal") => {
            Some(ComputedValue::Number(1.0))
        }
        Value::Keyword(keyword) if is_css_wide_keyword(&keyword.to_ascii_lowercase()) => {
            Some(ComputedValue::Keyword(keyword.to_ascii_lowercase()))
        }
        _ => numeric_zoom(value).map(ComputedValue::Number),
    };
    Some(computed.map_or(DeclarationValidation::Invalid, DeclarationValidation::Valid))
}
