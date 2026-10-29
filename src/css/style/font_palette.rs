//! Grammar and computed serialization for color font palette selection.
use super::{Value, individual_transform, is_css_wide_keyword, render_value, typed_math};

pub(super) fn valid(value: &Value) -> bool {
    if matches!(value, Value::Keyword(word) if is_css_wide_keyword(word)) {
        return true;
    }
    palette(value)
}

fn palette(value: &Value) -> bool {
    match value {
        Value::Keyword(word) => {
            matches!(word.as_str(), "normal" | "light" | "dark") || word.starts_with("--")
        }
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("palette-mix") => {
            let components = palette_arguments(arguments);
            !components.is_empty()
                && components.iter().all(|value| component(value).is_some())
                && (!has_interpolation(arguments) || interpolation(&arguments[0]).is_some())
        }
        _ => false,
    }
}

fn has_interpolation(arguments: &[Value]) -> bool {
    matches!(arguments.first(), Some(Value::List(values))
        if matches!(values.first(), Some(Value::Keyword(word)) if word.eq_ignore_ascii_case("in")))
}

fn palette_arguments(arguments: &[Value]) -> &[Value] {
    if has_interpolation(arguments) {
        &arguments[1..]
    } else {
        arguments
    }
}

fn interpolation(value: &Value) -> Option<String> {
    let Value::List(values) = value else {
        return None;
    };
    let [Value::Keyword(in_word), Value::Keyword(space), tail @ ..] = values.as_slice() else {
        return None;
    };
    if !in_word.eq_ignore_ascii_case("in") {
        return None;
    }
    let space = space.to_ascii_lowercase();
    let polar = matches!(space.as_str(), "hsl" | "hwb" | "lch" | "oklch");
    if !polar
        && !matches!(
            space.as_str(),
            "srgb"
                | "srgb-linear"
                | "lab"
                | "oklab"
                | "xyz"
                | "xyz-d50"
                | "xyz-d65"
                | "display-p3"
                | "a98-rgb"
                | "prophoto-rgb"
                | "rec2020"
        )
    {
        return None;
    }
    if tail.is_empty() {
        return Some(format!("in {space}"));
    }
    let [Value::Keyword(method), Value::Keyword(hue)] = tail else {
        return None;
    };
    if !polar
        || !hue.eq_ignore_ascii_case("hue")
        || !matches!(
            method.to_ascii_lowercase().as_str(),
            "shorter" | "longer" | "increasing" | "decreasing"
        )
    {
        return None;
    }
    Some(format!("in {space} {} hue", method.to_ascii_lowercase()))
}

fn component(value: &Value) -> Option<(&Value, Option<&Value>)> {
    if palette(value) {
        return Some((value, None));
    }
    let Value::List(values) = value else {
        return None;
    };
    let [first, second] = values.as_slice() else {
        return None;
    };
    let (palette, weight) = if palette(first) {
        (first, second)
    } else {
        (second, first)
    };
    if !self::palette(palette)
        || typed_math::infer(weight, None) != Some(typed_math::NumericType::PERCENTAGE)
    {
        return None;
    }
    if matches!(weight, Value::Percentage(value) if !(0.0..=100.0).contains(value)) {
        return None;
    }
    Some((palette, Some(weight)))
}

pub(super) fn compute(value: &Value) -> String {
    let Value::Function { name, arguments } = value else {
        return render_value(value);
    };
    if !name.eq_ignore_ascii_case("palette-mix") || !valid(value) {
        return "normal".into();
    }
    let Some(components) = palette_arguments(arguments)
        .iter()
        .map(component)
        .collect::<Option<Vec<_>>>()
    else {
        return "normal".into();
    };
    let palettes = components
        .iter()
        .map(|(palette, _)| compute(palette))
        .collect::<Vec<_>>();
    let weights = components
        .iter()
        .map(|(_, weight)| match weight {
            Some(weight) => individual_transform::fixed_percentage_expression(weight).map(Some),
            None => Some(None),
        })
        .collect::<Option<Vec<_>>>();
    let Some(mut weights) = weights else {
        return "normal".into();
    };
    let sum: f32 = weights.iter().flatten().sum();
    let omitted = weights.iter().filter(|weight| weight.is_none()).count();
    if omitted > 0 {
        let remainder = (100.0 - sum).max(0.0) / omitted as f32;
        for weight in &mut weights {
            if weight.is_none() {
                *weight = Some(remainder);
            }
        }
    }
    let weights = weights.into_iter().flatten().collect::<Vec<_>>();
    if weights.iter().sum::<f32>() <= 0.0 {
        return "normal".into();
    }
    // Below 100%, percentage normalization also reduces palette alpha.
    if weights.iter().sum::<f32>() >= 100.0
        && palettes.iter().all(|palette| palette == &palettes[0])
    {
        return palettes[0].clone();
    }
    let equal = weights.iter().all(|weight| *weight == weights[0])
        && weights[0] >= 100.0 / weights.len() as f32;
    let mut serialized = Vec::new();
    if has_interpolation(arguments) {
        let method = interpolation(&arguments[0]).unwrap();
        if method != "in oklab" {
            serialized.push(method);
        }
    }
    serialized.extend(palettes.into_iter().zip(weights).map(|(palette, weight)| {
        if equal {
            palette
        } else {
            format!("{palette} {weight}%")
        }
    }));
    format!("palette-mix({})", serialized.join(", "))
}

pub(super) fn mix_components(value: &Value) -> Option<(String, Vec<(&Value, f32)>)> {
    let Value::Function { name, arguments } = value else {
        return None;
    };
    if !name.eq_ignore_ascii_case("palette-mix") || !valid(value) {
        return None;
    }
    let components = palette_arguments(arguments)
        .iter()
        .map(component)
        .collect::<Option<Vec<_>>>()?;
    let mut weights = components
        .iter()
        .map(|(_, weight)| match weight {
            Some(weight) => individual_transform::fixed_percentage_expression(weight).map(Some),
            None => Some(None),
        })
        .collect::<Option<Vec<_>>>()?;
    let specified: f32 = weights.iter().flatten().sum();
    let omitted = weights.iter().filter(|weight| weight.is_none()).count();
    if omitted > 0 {
        let remainder = (100.0 - specified).max(0.0) / omitted as f32;
        for weight in &mut weights {
            if weight.is_none() {
                *weight = Some(remainder);
            }
        }
    }
    let weights = weights.into_iter().flatten().collect::<Vec<_>>();
    (weights.iter().sum::<f32>() > 0.0).then(|| {
        let method = if has_interpolation(arguments) {
            interpolation(&arguments[0]).unwrap()
        } else {
            "in oklab".to_string()
        };
        (
            method,
            components
                .into_iter()
                .zip(weights)
                .map(|((palette, _), weight)| (palette, weight))
                .collect(),
        )
    })
}
