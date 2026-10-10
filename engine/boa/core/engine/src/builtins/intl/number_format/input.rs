//! ToIntlMathematicalValue retains exact decimal inputs and nonfinite values.

use crate::{
    Context, JsNativeError, JsResult, JsValue,
    value::{JsVariant, PreferredType},
};
use fixed_decimal::{Decimal, FloatPrecision, Sign};

/// No floating-point conversion is required for native duration inputs.
#[derive(Clone, Debug)]
pub(crate) enum MathematicalValue {
    Finite(Decimal),
    NaN,
    Infinity { negative: bool },
}

impl MathematicalValue {
    pub(crate) fn from_f64(value: f64) -> JsResult<Self> {
        if value.is_nan() {
            return Ok(Self::NaN);
        }
        if value.is_infinite() {
            return Ok(Self::Infinity {
                negative: value.is_sign_negative(),
            });
        }
        Decimal::try_from_f64(value, FloatPrecision::RoundTrip)
            .map(Self::Finite)
            .map_err(|error| {
                JsNativeError::range()
                    .with_message(error.to_string())
                    .into()
            })
    }
}

pub(super) fn to_mathematical_value(
    value: &JsValue,
    context: &mut Context,
) -> JsResult<MathematicalValue> {
    let primitive = value.to_primitive(context, PreferredType::Number)?;
    match primitive.variant() {
        JsVariant::BigInt(value) => Decimal::try_from_str(&value.to_string())
            .map(MathematicalValue::Finite)
            .map_err(|error| {
                JsNativeError::range()
                    .with_message(error.to_string())
                    .into()
            }),
        JsVariant::String(value) => {
            let rounded = primitive.to_number(context)?;
            if !rounded.is_finite() {
                return MathematicalValue::from_f64(rounded);
            }
            // StringIntlMV rounds overflow/underflow to infinities and signed zero,
            // while ordinary finite strings retain their exact decimal digits.
            if rounded == 0.0 {
                let mut zero = Decimal::from(0);
                if rounded.is_sign_negative() {
                    zero.set_sign(Sign::Negative);
                }
                return Ok(MathematicalValue::Finite(zero));
            }
            Ok(super::js_string_to_fixed_decimal(&value)
                .map(MathematicalValue::Finite)
                .unwrap_or(MathematicalValue::NaN))
        }
        _ => MathematicalValue::from_f64(primitive.to_number(context)?),
    }
}
