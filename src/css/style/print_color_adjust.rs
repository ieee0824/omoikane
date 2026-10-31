//! Print-only paint snapshots using `print-color-adjust`.

use super::{
    ComputedStyle, ComputedValue, DeclarationValidation, Value, is_css_wide_keyword, render_value,
};
use crate::css::{MediaEnvironment, MediaType};
use std::sync::Arc;

pub(super) fn validate(name: &str, value: &Value) -> Option<DeclarationValidation> {
    if name != "print-color-adjust" {
        return None;
    }
    let valid = matches!(value, Value::Keyword(keyword) if
        matches!(keyword.to_ascii_lowercase().as_str(), "economy" | "exact")
        || is_css_wide_keyword(&keyword.to_ascii_lowercase()));
    Some(if valid {
        DeclarationValidation::Valid(ComputedValue::Keyword(render_value(value)))
    } else {
        DeclarationValidation::Invalid
    })
}

/// Applies the print-economy policy without changing observable computed style.
pub(super) fn paint_style(
    ordinary: Arc<ComputedStyle>,
    environment: &MediaEnvironment,
) -> Arc<ComputedStyle> {
    if environment.media_type != MediaType::Print
        || ordinary
            .get("print-color-adjust")
            .is_some_and(|value| value.css_text().eq_ignore_ascii_case("exact"))
    {
        return ordinary;
    }

    let has_background_color = ordinary
        .get("background-color")
        .and_then(|value| crate::paint::color::parse_color(&value.css_text()))
        .is_some_and(|color| color.a != 0);
    let has_background_image = ordinary
        .get("background-image")
        .is_some_and(|value| !value.css_text().eq_ignore_ascii_case("none"));
    if !has_background_color && !has_background_image {
        return ordinary;
    }

    let mut paint = ordinary.as_ref().clone();
    paint
        .properties
        .insert("background-color", ComputedValue::Color("#ffffff".into()));
    paint
        .properties
        .insert("background-image", ComputedValue::Keyword("none".into()));
    for property in [
        "color",
        "text-decoration-color",
        "-webkit-text-fill-color",
        "-webkit-text-stroke-color",
    ] {
        paint
            .properties
            .insert(property, ComputedValue::Color("#000000".into()));
    }
    Arc::new(paint)
}
