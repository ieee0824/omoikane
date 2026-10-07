//! Grammar, owned computed values, and serialization of individual transforms.
use super::*;
pub(super) mod context;
mod interpolation;
mod math;
pub(super) mod snapshot;

#[derive(Clone, Debug)]
pub(crate) enum IndividualTransform {
    None,
    Translate([ComputedValue; 3]),
    Rotate { axis: [f32; 3], degrees: f32 },
    Scale([f32; 3]),
}

pub(super) fn validate(property: &str, value: &Value) -> Option<DeclarationValidation> {
    if !matches!(property, "translate" | "rotate" | "scale") {
        return None;
    }
    if matches!(value, Value::Keyword(word) if is_css_wide_keyword(&word.to_ascii_lowercase())) {
        return Some(DeclarationValidation::Unvalidated);
    }
    Some(
        if parse(property, value, ResolutionContext::default()).is_some() {
            DeclarationValidation::Unvalidated
        } else {
            DeclarationValidation::Invalid
        },
    )
}

pub(super) fn compute(property: &str, value: &Value, context: ResolutionContext) -> String {
    parse(property, value, context).map_or_else(|| "none".into(), |value| value.serialize())
}

pub(crate) fn parse_computed(property: &str, text: &str) -> Option<IndividualTransform> {
    let declarations = super::super::parse_style_attribute(&format!("{property}:{text}"));
    parse(
        property,
        &declarations.first()?.value,
        ResolutionContext::default(),
    )
}

fn parse(property: &str, value: &Value, context: ResolutionContext) -> Option<IndividualTransform> {
    if matches!(value, Value::Keyword(word) if word.eq_ignore_ascii_case("none")) {
        return Some(IndividualTransform::None);
    }
    let values = match value {
        Value::List(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    match property {
        "translate" if (1..=3).contains(&values.len()) => {
            let mut coordinates = [
                ComputedValue::Px(0.0),
                ComputedValue::Px(0.0),
                ComputedValue::Px(0.0),
            ];
            for (index, value) in values.iter().enumerate() {
                coordinates[index] = translation_length(value, context, index != 2)?;
            }
            Some(IndividualTransform::Translate(coordinates))
        }
        "scale" if (1..=3).contains(&values.len()) => {
            let mut scales = [1.0; 3];
            for (index, value) in values.iter().enumerate() {
                let scalar = math::evaluate(value, context)?;
                scales[index] = match scalar.dimension {
                    math::Dimension::Number => scalar.value,
                    math::Dimension::Percentage => scalar.value / 100.0,
                    _ => return None,
                };
            }
            if values.len() == 1 {
                scales[1] = scales[0];
            }
            Some(IndividualTransform::Scale(scales))
        }
        "rotate" => parse_rotation(values, context),
        _ => None,
    }
}

fn translation_length(
    value: &Value,
    context: ResolutionContext,
    percentage: bool,
) -> Option<ComputedValue> {
    let computed = match value {
        Value::Number(number) if *number == 0.0 => ComputedValue::Px(0.0),
        Value::Length(number, unit) => {
            ComputedValue::Px(resolve_length_to_px(*number, unit, context)?)
        }
        Value::Percentage(number) if percentage => ComputedValue::Percentage(*number),
        Value::Function { .. } => {
            let expression = evaluate_length_percentage_math(value, context)?;
            if !percentage && !expression.is_pure_px() {
                return None;
            }
            computed_length_percentage_math(expression, "left", context)
        }
        _ => return None,
    };
    computed
        .resolve_length_percentage(0.0)?
        .is_finite()
        .then_some(computed)
}

fn parse_rotation(values: &[Value], context: ResolutionContext) -> Option<IndividualTransform> {
    let angles = values
        .iter()
        .enumerate()
        .filter_map(|(index, value)| {
            let scalar = math::evaluate(value, context)?;
            (scalar.dimension == math::Dimension::Angle).then_some((index, scalar.value))
        })
        .collect::<Vec<_>>();
    let [(angle_index, degrees)] = angles.as_slice() else {
        return None;
    };
    let rest = values
        .iter()
        .enumerate()
        .filter_map(|(index, value)| (index != *angle_index).then_some(value))
        .collect::<Vec<_>>();
    let axis = match rest.as_slice() {
        [] => [0.0, 0.0, 1.0],
        [Value::Keyword(axis)] => match axis.to_ascii_lowercase().as_str() {
            "x" => [1.0, 0.0, 0.0],
            "y" => [0.0, 1.0, 0.0],
            "z" => [0.0, 0.0, 1.0],
            _ => return None,
        },
        [x, y, z] => {
            // The three numbers must remain contiguous around the angle.
            if *angle_index != 0 && *angle_index != 3 {
                return None;
            }
            let number = |value| {
                let scalar = math::evaluate(value, context)?;
                (scalar.dimension == math::Dimension::Number).then_some(scalar.value)
            };
            [number(x)?, number(y)?, number(z)?]
        }
        _ => return None,
    };
    Some(IndividualTransform::Rotate {
        axis,
        degrees: *degrees,
    })
}

impl IndividualTransform {
    pub(crate) fn serialize(&self) -> String {
        match self {
            Self::None => "none".into(),
            Self::Translate(values) => {
                let mut count = 3;
                while count > 1 && values[count - 1] == ComputedValue::Px(0.0) {
                    count -= 1;
                }
                values[..count]
                    .iter()
                    .map(length_text)
                    .collect::<Vec<_>>()
                    .join(" ")
            }
            Self::Scale(values) => {
                let count = if values[2] != 1.0 {
                    3
                } else if values[0] != values[1] {
                    2
                } else {
                    1
                };
                values[..count]
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" ")
            }
            Self::Rotate { axis, degrees } => rotation_text(*axis, &format!("{degrees}deg"), false),
        }
    }
}

fn length_text(value: &ComputedValue) -> String {
    if let Some((px, percent)) = value.linear_length_percentage_components() {
        if px != 0.0 && percent != 0.0 {
            return format!(
                "calc({percent}% {} {}px)",
                if px < 0.0 { "-" } else { "+" },
                px.abs()
            );
        }
    }
    value.css_text()
}

fn canonical_axis(axis: [f32; 3]) -> (Option<&'static str>, bool) {
    match axis {
        [x, 0.0, 0.0] if x != 0.0 => (Some("x"), x < 0.0),
        [0.0, y, 0.0] if y != 0.0 => (Some("y"), y < 0.0),
        [0.0, 0.0, z] if z != 0.0 => (Some("z"), z < 0.0),
        _ => (None, false),
    }
}

fn rotation_text(axis: [f32; 3], angle: &str, already_signed: bool) -> String {
    let (keyword, reverse) = canonical_axis(axis);
    let angle = if reverse && !already_signed {
        negate_angle_text(angle)
    } else {
        angle.to_string()
    };
    match keyword {
        Some("z") => angle,
        Some(keyword) => format!("{keyword} {angle}"),
        None => format!("{} {} {} {angle}", axis[0], axis[1], axis[2]),
    }
}

fn negate_angle_text(angle: &str) -> String {
    let declarations = super::super::parse_style_attribute(&format!("rotate:{angle}"));
    match declarations.first().map(|d| &d.value) {
        Some(Value::Length(number, unit)) => format!("{}{unit}", -number),
        _ => format!("calc(-1 * {angle})"),
    }
}

pub(crate) fn normalize_specified(property: &str, text: &str) -> Option<String> {
    if !supports_declaration(property, text) {
        return None;
    }
    let declarations = super::super::parse_style_attribute(&format!("{property}:{text}"));
    let value = &declarations.first()?.value;
    if matches!(value, Value::Keyword(word) if is_css_wide_keyword(&word.to_ascii_lowercase()) || word.eq_ignore_ascii_case("none"))
    {
        return Some(render_value(value).to_ascii_lowercase());
    }
    let parsed = parse(property, value, ResolutionContext::default())?;
    let values = match value {
        Value::List(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    match parsed {
        IndividualTransform::Rotate { axis, .. } => {
            let angle = values.iter().find(|value| {
                math::evaluate(value, ResolutionContext::default())
                    .is_some_and(|v| v.dimension == math::Dimension::Angle)
            })?;
            Some(rotation_text(axis, &render_value(angle), false))
        }
        IndividualTransform::Translate(_) => {
            let mut values = values.iter().map(specified_length).collect::<Vec<_>>();
            while values.len() > 1 && values.last().is_some_and(|value| value == "0px") {
                values.pop();
            }
            Some(values.join(" "))
        }
        IndividualTransform::Scale(scales) => {
            let mut values = values
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    if matches!(value, Value::Function { .. }) {
                        specified_math(value)
                    } else {
                        scales[index].to_string()
                    }
                })
                .collect::<Vec<_>>();
            if values.len() == 3 && values[2] == "1" {
                values.pop();
            }
            if values.len() == 2 && values[0] == values[1] {
                values.pop();
            }
            Some(values.join(" "))
        }
        IndividualTransform::None => Some("none".into()),
    }
}

fn specified_length(value: &Value) -> String {
    if matches!(value, Value::Number(number) if *number == 0.0) {
        return "0px".into();
    }
    if matches!(value, Value::Function { .. }) {
        if let Some(math) = evaluate_length_percentage_math(value, ResolutionContext::default()) {
            let text = render_value(value);
            if !has_relative_length(value)
                && let Some((px, percent)) = math.linear_components()
            {
                if px != 0.0 && percent != 0.0 {
                    return length_text(&ComputedValue::LengthPercentage(math));
                }
            }
            return text;
        }
    }
    render_value(value)
}

fn has_relative_length(value: &Value) -> bool {
    match value {
        Value::Length(_, unit) => !matches!(
            unit.to_ascii_lowercase().as_str(),
            "px" | "deg" | "grad" | "rad" | "turn"
        ),
        Value::List(values) => values.iter().any(has_relative_length),
        Value::Function { arguments, .. } => arguments.iter().any(has_relative_length),
        _ => false,
    }
}

fn specified_math(value: &Value) -> String {
    if has_relative_length(value) {
        return render_value(value);
    }
    if let Some(scalar) = math::evaluate(value, ResolutionContext::default()) {
        let unit = match scalar.dimension {
            math::Dimension::Percentage => "%",
            _ => "",
        };
        return format!("calc({}{unit})", scalar.value);
    }
    render_value(value)
}

pub(crate) fn interpolate(property: &str, start: &str, end: &str, progress: f32) -> Option<String> {
    interpolation::interpolate(
        property,
        parse_computed(property, start)?,
        parse_computed(property, end)?,
        progress,
    )
}
