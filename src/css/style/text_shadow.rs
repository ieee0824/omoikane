//! Text shadow grammar and computed lengths. Currentcolor stays symbolic until
//! CSSOM or paint supplies the element's own color, including after inheritance.

use super::*;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TextShadow {
    pub(crate) offset_x: f32,
    pub(crate) offset_y: f32,
    pub(crate) blur: f32,
    pub(crate) color: String,
}

struct SpecifiedShadow {
    lengths: Vec<Value>,
    color: Option<Value>,
}

fn length(value: &Value, ctx: ResolutionContext) -> Option<f32> {
    match value {
        Value::Length(number, unit) if number.is_finite() && is_css_length_unit(unit) => {
            resolve_length_to_px(*number, unit, ctx).filter(|n| n.is_finite())
        }
        Value::Number(number) if *number == 0.0 => Some(0.0),
        Value::Function { name, .. } if is_length_percentage_math_function(name) => {
            match compute_value(value, "--shadow-length", ctx) {
                ComputedValue::Px(number) if number.is_finite() => Some(number),
                _ => None,
            }
        }
        _ => None,
    }
}

fn specified_layer(value: &Value) -> Option<SpecifiedShadow> {
    let components = match value {
        Value::List(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    let mut lengths = Vec::new();
    let mut color = None;
    for (index, component) in components.iter().enumerate() {
        if length(component, ResolutionContext::default()).is_some() {
            lengths.push(component.clone());
        } else {
            if color.is_some()
                || (index != 0 && index + 1 != components.len())
                || matches!(component, Value::Keyword(k) if is_css_wide_keyword(&k.to_ascii_lowercase()))
                || !is_valid_color_value(component)
            {
                return None;
            }
            color = Some(component.clone());
        }
    }
    if !(2..=3).contains(&lengths.len()) {
        return None;
    }
    // Math expressions use CSS's nonnegative range clamp at computed time.
    if let Some(Value::Length(number, _) | Value::Number(number)) = lengths.get(2)
        && *number < 0.0
    {
        return None;
    }
    Some(SpecifiedShadow { lengths, color })
}

fn specified(value: &Value) -> Option<Vec<SpecifiedShadow>> {
    if matches!(value, Value::Keyword(k) if k.eq_ignore_ascii_case("none")) {
        return Some(Vec::new());
    }
    let layers = match value {
        Value::CommaList(layers) => layers.as_slice(),
        value => std::slice::from_ref(value),
    };
    if layers.is_empty() {
        return None;
    }
    layers.iter().map(specified_layer).collect()
}

pub(super) fn validate(value: &Value) -> DeclarationValidation {
    if matches!(value, Value::Keyword(k) if is_css_wide_keyword(&k.to_ascii_lowercase())) {
        return DeclarationValidation::Valid(ComputedValue::Keyword(
            render_value(value).to_ascii_lowercase(),
        ));
    }
    if specified(value).is_some() {
        DeclarationValidation::Unvalidated
    } else {
        DeclarationValidation::Invalid
    }
}

/// Validates CSSOM assignments without resolving font-relative specified lengths.
pub(crate) fn normalize_specified(text: &str) -> Option<String> {
    if !supports_declaration("text-shadow", text) {
        return None;
    }
    let declarations = super::super::parse_style_attribute(&format!("text-shadow:{text}"));
    let value = &declarations.first()?.value;
    if value_contains_var_function(value) {
        return Some(render_value(value));
    }
    if let DeclarationValidation::Valid(computed) = validate(value) {
        return Some(computed.css_text());
    }
    let shadows = specified(value)?;
    if shadows.is_empty() {
        return Some("none".into());
    }
    Some(
        shadows
            .into_iter()
            .map(|shadow| {
                let mut components = Vec::new();
                if let Some(color) = shadow.color {
                    let text = render_value(&color);
                    components.push(crate::paint::color4::CssColor::parse(&text).map_or_else(
                        || {
                            let computed =
                                compute_value(&color, "color", ResolutionContext::default())
                                    .css_text();
                            if matches!(color, Value::Keyword(_)) {
                                computed
                            } else {
                                computed_color_text(&computed).unwrap_or(computed)
                            }
                        },
                        |color| color.serialize(),
                    ));
                }
                components.extend(shadow.lengths.iter().map(|value| {
                    if matches!(value, Value::Number(n) if *n == 0.0) {
                        "0px".into()
                    } else {
                        render_value(value)
                    }
                }));
                components.join(" ")
            })
            .collect::<Vec<_>>()
            .join(", "),
    )
}

pub(super) fn compute(value: &Value, ctx: ResolutionContext) -> Option<ComputedValue> {
    let shadows = specified(value)?
        .into_iter()
        .map(|shadow| {
            let color = shadow.color.map_or_else(
                || "currentcolor".to_string(),
                |value| compute_value(&value, "color", ctx).css_text(),
            );
            Some(TextShadow {
                offset_x: length(&shadow.lengths[0], ctx)?,
                offset_y: length(&shadow.lengths[1], ctx)?,
                blur: match shadow.lengths.get(2) {
                    Some(v) => length(v, ctx)?,
                    None => 0.0,
                }
                .max(0.0),
                color,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(ComputedValue::Keyword(serialize(&shadows)))
}

pub(crate) fn parse_computed(text: &str) -> Option<Vec<TextShadow>> {
    if text.trim().eq_ignore_ascii_case("none") {
        return Some(Vec::new());
    }
    let declaration = super::super::parse_style_attribute(&format!("text-shadow:{text}"));
    let value = &declaration.first()?.value;
    specified(value)?
        .into_iter()
        .map(|shadow| {
            let ctx = ResolutionContext::default();
            Some(TextShadow {
                offset_x: length(&shadow.lengths[0], ctx)?,
                offset_y: length(&shadow.lengths[1], ctx)?,
                blur: match shadow.lengths.get(2) {
                    Some(v) => length(v, ctx)?,
                    None => 0.0,
                }
                .max(0.0),
                color: shadow
                    .color
                    .as_ref()
                    .map(render_value)
                    .unwrap_or_else(|| "currentcolor".into()),
            })
        })
        .collect()
}

pub(crate) fn serialize(shadows: &[TextShadow]) -> String {
    if shadows.is_empty() {
        return "none".into();
    }
    shadows
        .iter()
        .map(|s| {
            format!(
                "{} {}px {}px {}px",
                s.color,
                canonical_zero(s.offset_x),
                canonical_zero(s.offset_y),
                canonical_zero(s.blur)
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn canonical_zero(value: f32) -> f32 {
    if value == 0.0 { 0.0 } else { value }
}

pub(crate) fn resolved_css_text(text: &str, current_color: &str) -> Option<String> {
    let mut shadows = parse_computed(text)?;
    for shadow in &mut shadows {
        let text = if shadow.color.eq_ignore_ascii_case("currentcolor") {
            current_color
        } else {
            &shadow.color
        };
        shadow.color = computed_color_text(text)?;
    }
    Some(serialize(&shadows))
}

/// Interpolates shadow lists, padding the shorter list with transparent shadows.
/// The explicit color context resolves currentcolor only while sampling.
pub(crate) fn interpolate(
    start: &str,
    end: &str,
    progress: f32,
    current_color: &str,
) -> Option<String> {
    let start = parse_computed(start)?;
    let end = parse_computed(end)?;
    let identity = TextShadow {
        offset_x: 0.0,
        offset_y: 0.0,
        blur: 0.0,
        color: "transparent".into(),
    };
    let mix = |a: f32, b: f32| a + (b - a) * progress;
    let shadows = (0..start.len().max(end.len()))
        .map(|index| {
            let a = start.get(index).unwrap_or(&identity);
            let b = end.get(index).unwrap_or(&identity);
            Some(TextShadow {
                offset_x: mix(a.offset_x, b.offset_x),
                offset_y: mix(a.offset_y, b.offset_y),
                blur: mix(a.blur, b.blur).max(0.0),
                color: interpolate_shadow_color(&a.color, &b.color, progress, current_color)?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(serialize(&shadows))
}

fn interpolate_shadow_color(
    start: &str,
    end: &str,
    progress: f32,
    current_color: &str,
) -> Option<String> {
    if start.eq_ignore_ascii_case("currentcolor") && end.eq_ignore_ascii_case("currentcolor") {
        return Some("currentcolor".into());
    }
    let resolve = |text: &str| {
        let text = if text.eq_ignore_ascii_case("currentcolor") {
            current_color
        } else {
            text
        };
        let color = crate::paint::color::parse_color(text)?;
        // Computed rgba retains its unquantized alpha; painting alone quantizes it.
        let alpha = if let Some(color) = crate::paint::color4::CssColor::parse(text) {
            color.alpha.unwrap_or(0.0) as f32
        } else if text.starts_with("rgba(") {
            text.trim_end_matches(')')
                .rsplit_once(',')?
                .1
                .trim()
                .parse::<f32>()
                .ok()?
        } else {
            f32::from(color.a) / 255.0
        };
        Some((color, alpha))
    };
    let (a, a_alpha) = resolve(start)?;
    let (b, b_alpha) = resolve(end)?;
    let raw_alpha = a_alpha + (b_alpha - a_alpha) * progress;
    let channel = |a: u8, b: u8| {
        if raw_alpha <= 0.0 {
            0.0
        } else {
            (f32::from(a) * a_alpha * (1.0 - progress) + f32::from(b) * b_alpha * progress)
                / raw_alpha
        }
        .clamp(0.0, 255.0)
        .round() as u8
    };
    let (r, g, b) = (channel(a.r, b.r), channel(a.g, b.g), channel(a.b, b.b));
    let alpha = raw_alpha.clamp(0.0, 1.0);
    Some(if alpha == 1.0 {
        format!("rgb({r}, {g}, {b})")
    } else {
        format!("rgba({r}, {g}, {b}, {alpha})")
    })
}

fn computed_color_text(text: &str) -> Option<String> {
    if let Some(color) = crate::paint::color4::CssColor::parse(text) {
        return Some(color.serialize_computed());
    }
    let color = crate::paint::color::parse_color(text)?;
    if text.starts_with("rgba(") {
        return Some(text.to_string());
    }
    Some(if color.a == 255 {
        format!("rgb({}, {}, {})", color.r, color.g, color.b)
    } else {
        format!(
            "rgba({}, {}, {}, {})",
            color.r,
            color.g,
            color.b,
            f32::from(color.a) / 255.0
        )
    })
}
