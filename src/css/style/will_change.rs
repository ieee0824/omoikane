//! Grammar validation for the stacking-context hint consumed by paint.
use super::{ComputedValue, DeclarationValidation, Value, is_css_wide_keyword, render_value};

pub(super) fn validate(name: &str, value: &Value) -> Option<DeclarationValidation> {
    if name != "will-change" {
        return None;
    }
    let valid = match value {
        Value::Keyword(name) => {
            name.eq_ignore_ascii_case("auto")
                || is_css_wide_keyword(&name.to_ascii_lowercase())
                || is_feature(name)
        }
        Value::CommaList(features) => {
            !features.is_empty()
                && features
                    .iter()
                    .all(|feature| matches!(feature, Value::Keyword(name) if is_feature(name)))
        }
        _ => false,
    };
    Some(if valid {
        DeclarationValidation::Valid(ComputedValue::Keyword(render_value(value)))
    } else {
        DeclarationValidation::Invalid
    })
}

fn is_feature(name: &str) -> bool {
    if name.is_empty()
        || name.eq_ignore_ascii_case("auto")
        || ["default", "will-change", "none", "all"]
            .iter()
            .any(|keyword| name.eq_ignore_ascii_case(keyword))
        || is_css_wide_keyword(&name.to_ascii_lowercase())
    {
        return false;
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    (first.is_alphabetic() || matches!(first, '_' | '-'))
        && chars.all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '-'))
}
