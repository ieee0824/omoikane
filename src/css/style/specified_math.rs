//! Context-independent simplification of specified numeric expressions.

use super::{Value, individual_transform, sibling_functions};

/// Folds fixed angle factors while retaining every element-dependent factor.
/// Expressions requiring fonts, percentages or DOM counts are never evaluated.
pub(super) fn fold_fixed_angle_product(value: &Value) -> Option<Value> {
    let Value::List(values) = value else {
        return None;
    };
    if values.len() < 3
        || values.len() % 2 == 0
        || !values
            .iter()
            .skip(1)
            .step_by(2)
            .all(|value| matches!(value, Value::Keyword(operator) if operator == "*"))
    {
        return None;
    }
    let (dynamic, fixed): (Vec<_>, Vec<_>) = values
        .iter()
        .step_by(2)
        .cloned()
        .partition(sibling_functions::contains);
    if dynamic.is_empty() || fixed.is_empty() {
        return None;
    }
    let angle = individual_transform::fixed_angle_expression(&product(fixed))?;
    Some(product(std::iter::once(angle).chain(dynamic)))
}

fn product(values: impl IntoIterator<Item = Value>) -> Value {
    let mut result = Vec::new();
    for value in values {
        if !result.is_empty() {
            result.push(Value::Keyword("*".into()));
        }
        result.push(value);
    }
    if result.len() == 1 {
        result.pop().unwrap()
    } else {
        Value::List(result)
    }
}
