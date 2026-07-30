//! CSS shorthand property expansion.

use super::{Declaration, Value};

pub(super) fn expand_shorthand(name: &str, value: Value, important: bool) -> Vec<Declaration> {
    match name {
        "margin" | "padding" => expand_box_shorthand(name, value, important),
        "margin-inline" | "margin-block" | "padding-inline" | "padding-block" => {
            expand_logical_axis_shorthand(name, value, important)
        }
        "border-width" | "border-style" | "border-color" => {
            expand_border_axis_shorthand(name, value, important)
        }
        "border" => expand_border_shorthand(value, important),
        "border-top" | "border-right" | "border-bottom" | "border-left" => {
            expand_border_side_shorthand(name, value, important)
        }
        "background" => expand_background_shorthand(value, important),
        "background-position" => expand_background_position_shorthand(value, important),
        "mask" | "-webkit-mask" => expand_mask_shorthand(value, important),
        "mask-position" | "-webkit-mask-position" => {
            expand_mask_position_shorthand(value, important)
        }
        "font" => expand_font_shorthand(value, important),
        "overflow" => expand_overflow_shorthand(value, important),
        "flex" => expand_flex_shorthand(value, important),
        "text-decoration" => expand_text_decoration_shorthand(value, important),
        "border-radius" => expand_border_radius_shorthand(value, important),
        "box-shadow" => expand_box_shadow_shorthand(value, important),
        "list-style" => expand_list_style_shorthand(value, important),
        "flex-flow" => expand_flex_flow_shorthand(value, important),
        "animation" => expand_animation_shorthand(value, important),
        "transition" => super::expand_transition_shorthand(value, important),
        "outline" => expand_outline_shorthand(value, important),
        "grid-column" | "grid-row" => expand_grid_axis_shorthand(name, value, important),
        "grid-area" => expand_grid_area_shorthand(value, important),
        "grid-template" => expand_grid_template_shorthand(value, important),
        "place-items" => expand_place_shorthand("align-items", "justify-items", value, important),
        "place-self" => expand_place_shorthand("align-self", "justify-self", value, important),
        "place-content" => expand_place_shorthand("align-content", "justify-content", value, important),
        // `word-wrap` is a legacy alias for `overflow-wrap`
        "word-wrap" => vec![Declaration {
            name: "overflow-wrap".to_string(),
            value,
            important,
        }],
        _ => vec![Declaration {
            name: name.to_string(),
            value,
            important,
        }],
    }
}

fn expand_background_position_shorthand(value: Value, important: bool) -> Vec<Declaration> {
    let original = value.clone();
    let layers = match value {
        Value::CommaList(layers) => layers,
        single => vec![single],
    };
    let mut x_values = Vec::new();
    let mut y_values = Vec::new();
    for layer in layers {
        let components = match layer {
            Value::List(values) if (1..=2).contains(&values.len()) => values,
            single @ (Value::Keyword(_)
            | Value::Length(_, _)
            | Value::Percentage(_)) => vec![single],
            Value::Number(number) if number == 0.0 => vec![Value::Number(number)],
            _ => {
                return vec![Declaration {
                    name: "background-position".to_string(),
                    value: original,
                    important,
                }];
            }
        };
        let Some((x, y)) = normalize_background_position(&components) else {
            return vec![Declaration {
                name: "background-position".to_string(),
                value: original,
                important,
            }];
        };
        x_values.push(x);
        y_values.push(y);
    }
    let layered = |values: Vec<Value>| {
        if values.len() == 1 {
            values.into_iter().next().expect("one background layer")
        } else {
            Value::CommaList(values)
        }
    };
    vec![
        Declaration {
            name: "background-position-x".to_string(),
            value: layered(x_values),
            important,
        },
        Declaration {
            name: "background-position-y".to_string(),
            value: layered(y_values),
            important,
        },
    ]
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BackgroundPositionAxis {
    Horizontal,
    Vertical,
    Center,
    Unspecified,
}

fn background_position_axis(value: &Value) -> BackgroundPositionAxis {
    let Value::Keyword(keyword) = value else {
        return BackgroundPositionAxis::Unspecified;
    };
    match keyword.to_ascii_lowercase().as_str() {
        "left" | "right" => BackgroundPositionAxis::Horizontal,
        "top" | "bottom" => BackgroundPositionAxis::Vertical,
        "center" => BackgroundPositionAxis::Center,
        _ => BackgroundPositionAxis::Unspecified,
    }
}

fn normalize_background_position(values: &[Value]) -> Option<(Value, Value)> {
    let center = || Value::Keyword("center".to_string());
    match values {
        [value] if background_position_axis(value) == BackgroundPositionAxis::Vertical => {
            Some((center(), value.clone()))
        }
        [value] => Some((value.clone(), center())),
        [first, second] => {
            let first_axis = background_position_axis(first);
            let second_axis = background_position_axis(second);
            match (first_axis, second_axis) {
                (BackgroundPositionAxis::Horizontal, BackgroundPositionAxis::Horizontal)
                | (BackgroundPositionAxis::Vertical, BackgroundPositionAxis::Vertical) => None,
                (BackgroundPositionAxis::Horizontal, BackgroundPositionAxis::Vertical)
                | (BackgroundPositionAxis::Horizontal, BackgroundPositionAxis::Center)
                | (BackgroundPositionAxis::Center, BackgroundPositionAxis::Vertical) => {
                    Some((first.clone(), second.clone()))
                }
                (BackgroundPositionAxis::Vertical, BackgroundPositionAxis::Horizontal)
                | (BackgroundPositionAxis::Center, BackgroundPositionAxis::Horizontal) => {
                    Some((second.clone(), first.clone()))
                }
                (BackgroundPositionAxis::Vertical, BackgroundPositionAxis::Center) => {
                    Some((second.clone(), first.clone()))
                }
                (BackgroundPositionAxis::Horizontal, BackgroundPositionAxis::Unspecified) => {
                    Some((first.clone(), second.clone()))
                }
                (BackgroundPositionAxis::Vertical, BackgroundPositionAxis::Unspecified)
                | (BackgroundPositionAxis::Unspecified, BackgroundPositionAxis::Horizontal) => None,
                (BackgroundPositionAxis::Unspecified, BackgroundPositionAxis::Vertical) => {
                    Some((first.clone(), second.clone()))
                }
                _ => Some((first.clone(), second.clone())),
            }
        }
        _ => None,
    }
}

fn expand_logical_axis_shorthand(
    name: &str,
    value: Value,
    important: bool,
) -> Vec<Declaration> {
    let values = match value {
        Value::List(values) => values,
        value => vec![value],
    };
    let (start, end) = match values.as_slice() {
        [value] => (value.clone(), value.clone()),
        [start, end] => (start.clone(), end.clone()),
        _ => return Vec::new(),
    };
    let (start_suffix, end_suffix) = if name.ends_with("-inline") {
        ("inline-start", "inline-end")
    } else {
        ("block-start", "block-end")
    };
    let prefix = name.split('-').next().unwrap_or(name);
    vec![
        Declaration {
            name: format!("{prefix}-{start_suffix}"),
            value: start,
            important,
        },
        Declaration {
            name: format!("{prefix}-{end_suffix}"),
            value: end,
            important,
        },
    ]
}

fn expand_place_shorthand(
    first_name: &str,
    second_name: &str,
    value: Value,
    important: bool,
) -> Vec<Declaration> {
    let values = match value {
        Value::List(values) => values,
        value => vec![value],
    };
    let (first, second) = match values.as_slice() {
        [value] => (value.clone(), value.clone()),
        [first, second] => (first.clone(), second.clone()),
        _ => return Vec::new(),
    };
    vec![
        Declaration { name: first_name.to_string(), value: first, important },
        Declaration { name: second_name.to_string(), value: second, important },
    ]
}

fn split_compact_grid_slash(values: Vec<Value>) -> Vec<Value> {
    values
        .into_iter()
        .flat_map(|value| match value {
            Value::Keyword(keyword) if keyword.contains('/') && keyword != "/" => {
                let mut parts = keyword.splitn(2, '/');
                let before = parts.next().unwrap_or_default();
                let after = parts.next().unwrap_or_default();
                let mut values = Vec::new();
                if !before.is_empty() {
                    values.push(Value::Keyword(before.to_string()));
                }
                values.push(Value::Keyword("/".to_string()));
                if !after.is_empty() {
                    values.push(Value::Keyword(after.to_string()));
                }
                values
            }
            value => vec![value],
        })
        .collect()
}

fn expand_grid_axis_shorthand(name: &str, value: Value, important: bool) -> Vec<Declaration> {
    let values = match value {
        Value::List(values) => values,
        value => vec![value],
    };
    let values = split_compact_grid_slash(values);
    let slash = values.iter().position(|value| matches!(value, Value::Keyword(keyword) if keyword == "/"));
    let (start, end) = match slash {
        Some(index) if index > 0 && index + 1 < values.len() => (
            collapse_grid_line(&values[..index]),
            collapse_grid_line(&values[index + 1..]),
        ),
        Some(_) => return Vec::new(),
        None => (collapse_grid_line(&values), Value::Keyword("auto".to_string())),
    };
    vec![
        Declaration { name: format!("{name}-start"), value: start, important },
        Declaration { name: format!("{name}-end"), value: end, important },
    ]
}

fn collapse_grid_line(values: &[Value]) -> Value {
    match values {
        [value] => value.clone(),
        values => Value::List(values.to_vec()),
    }
}

fn expand_grid_area_shorthand(value: Value, important: bool) -> Vec<Declaration> {
    let values = match value {
        Value::List(values) => values,
        value => vec![value],
    };
    let mut parts: Vec<Vec<Value>> = vec![Vec::new()];
    for value in values {
        if matches!(&value, Value::Keyword(keyword) if keyword == "/") {
            if parts.last().is_some_and(Vec::is_empty) || parts.len() == 4 {
                return Vec::new();
            }
            parts.push(Vec::new());
        } else {
            parts.last_mut().expect("grid area has one part").push(value);
        }
    }
    if parts.last().is_some_and(Vec::is_empty) {
        return Vec::new();
    }

    let row_start = collapse_grid_line(&parts[0]);
    let column_start = parts.get(1).map(|part| collapse_grid_line(part)).unwrap_or_else(|| {
        custom_grid_identifier(&row_start).unwrap_or_else(auto_grid_line)
    });
    let row_end = parts.get(2).map(|part| collapse_grid_line(part)).unwrap_or_else(|| {
        custom_grid_identifier(&row_start).unwrap_or_else(auto_grid_line)
    });
    let column_end = parts.get(3).map(|part| collapse_grid_line(part)).unwrap_or_else(|| {
        custom_grid_identifier(&column_start).unwrap_or_else(auto_grid_line)
    });

    [
        ("grid-row-start", row_start),
        ("grid-column-start", column_start),
        ("grid-row-end", row_end),
        ("grid-column-end", column_end),
    ]
    .into_iter()
    .map(|(name, value)| Declaration { name: name.to_string(), value, important })
    .collect()
}

fn custom_grid_identifier(value: &Value) -> Option<Value> {
    let Value::Keyword(keyword) = value else { return None; };
    if keyword.eq_ignore_ascii_case("auto")
        || keyword.eq_ignore_ascii_case("span")
        || keyword.parse::<isize>().is_ok()
        || keyword.split_whitespace().count() != 1
    {
        None
    } else {
        Some(value.clone())
    }
}

fn auto_grid_line() -> Value { Value::Keyword("auto".to_string()) }

fn expand_grid_template_shorthand(value: Value, important: bool) -> Vec<Declaration> {
    let values = match value {
        Value::List(values) => values,
        value => vec![value],
    };
    let values = split_compact_grid_slash(values);
    let slash = values
        .iter()
        .position(|value| matches!(value, Value::Keyword(keyword) if keyword == "/"));
    let (before, columns) = match slash {
        Some(index) if index > 0 && index + 1 < values.len() => {
            (&values[..index], collapse_grid_line(&values[index + 1..]))
        }
        Some(_) => return Vec::new(),
        None => (&values[..], Value::Keyword("none".to_string())),
    };

    if !before.iter().any(|value| matches!(value, Value::String(_))) {
        if slash.is_none() {
            return Vec::new();
        }
        return vec![
            Declaration {
                name: "grid-template-rows".to_string(),
                value: collapse_grid_line(before),
                important,
            },
            Declaration { name: "grid-template-columns".to_string(), value: columns, important },
        ];
    }

    let mut areas = Vec::new();
    let mut rows = Vec::new();
    let mut index = 0;
    while index < before.len() {
        let Value::String(row) = &before[index] else { return Vec::new(); };
        areas.push(Value::String(row.clone()));
        index += 1;
        if index < before.len() && !matches!(before[index], Value::String(_)) {
            rows.push(before[index].clone());
            index += 1;
        } else {
            rows.push(Value::Keyword("auto".to_string()));
        }
    }

    vec![
        Declaration {
            name: "grid-template-areas".to_string(),
            value: Value::List(areas),
            important,
        },
        Declaration {
            name: "grid-template-rows".to_string(),
            value: Value::List(rows),
            important,
        },
        Declaration { name: "grid-template-columns".to_string(), value: columns, important },
    ]
}

fn expand_box_shorthand(prefix: &str, value: Value, important: bool) -> Vec<Declaration> {
    let values = match value {
        Value::List(values) => values,
        single => vec![single],
    };

    let (top, right, bottom, left) = match values.as_slice() {
        [a] => (a.clone(), a.clone(), a.clone(), a.clone()),
        [a, b] => (a.clone(), b.clone(), a.clone(), b.clone()),
        [a, b, c] => (a.clone(), b.clone(), c.clone(), b.clone()),
        [a, b, c, d] => (a.clone(), b.clone(), c.clone(), d.clone()),
        _ => {
            return vec![Declaration {
                name: prefix.to_string(),
                value: Value::List(values),
                important,
            }];
        }
    };

    vec![
        Declaration {
            name: format!("{prefix}-top"),
            value: top,
            important,
        },
        Declaration {
            name: format!("{prefix}-right"),
            value: right,
            important,
        },
        Declaration {
            name: format!("{prefix}-bottom"),
            value: bottom,
            important,
        },
        Declaration {
            name: format!("{prefix}-left"),
            value: left,
            important,
        },
    ]
}

fn expand_border_radius_shorthand(value: Value, important: bool) -> Vec<Declaration> {
    let values = match value {
        Value::List(values) => values,
        single => vec![single],
    };

    // CSS border-radius shorthand order: TL TR BR BL
    // 1値: 全角丸同じ
    // 2値: TL/BR = 1st, TR/BL = 2nd
    // 3値: TL=1st, TR/BL=2nd, BR=3rd
    // 4値: TL/TR/BR/BL それぞれ指定
    let (tl, tr, br, bl) = match values.as_slice() {
        [a] => (a.clone(), a.clone(), a.clone(), a.clone()),
        [a, b] => (a.clone(), b.clone(), a.clone(), b.clone()),
        [a, b, c] => (a.clone(), b.clone(), c.clone(), b.clone()),
        [a, b, c, d] => (a.clone(), b.clone(), c.clone(), d.clone()),
        _ => {
            return vec![Declaration {
                name: "border-radius".to_string(),
                value: Value::List(values),
                important,
            }];
        }
    };

    vec![
        Declaration {
            name: "border-top-left-radius".to_string(),
            value: tl,
            important,
        },
        Declaration {
            name: "border-top-right-radius".to_string(),
            value: tr,
            important,
        },
        Declaration {
            name: "border-bottom-right-radius".to_string(),
            value: br,
            important,
        },
        Declaration {
            name: "border-bottom-left-radius".to_string(),
            value: bl,
            important,
        },
    ]
}

fn expand_border_axis_shorthand(name: &str, value: Value, important: bool) -> Vec<Declaration> {
    let values = match value {
        Value::List(values) => values,
        single => vec![single],
    };

    let suffix = name.strip_prefix("border-").unwrap_or(name);
    let (top, right, bottom, left) = match values.as_slice() {
        [a] => (a.clone(), a.clone(), a.clone(), a.clone()),
        [a, b] => (a.clone(), b.clone(), a.clone(), b.clone()),
        [a, b, c] => (a.clone(), b.clone(), c.clone(), b.clone()),
        [a, b, c, d] => (a.clone(), b.clone(), c.clone(), d.clone()),
        _ => {
            return vec![Declaration {
                name: name.to_string(),
                value: Value::List(values),
                important,
            }];
        }
    };

    vec![
        Declaration {
            name: format!("border-top-{suffix}"),
            value: top,
            important,
        },
        Declaration {
            name: format!("border-right-{suffix}"),
            value: right,
            important,
        },
        Declaration {
            name: format!("border-bottom-{suffix}"),
            value: bottom,
            important,
        },
        Declaration {
            name: format!("border-left-{suffix}"),
            value: left,
            important,
        },
    ]
}

fn expand_border_shorthand(value: Value, important: bool) -> Vec<Declaration> {
    let values = match value {
        Value::List(values) => values,
        single => vec![single],
    };

    let mut width = None;
    let mut style = None;
    let mut color = None;

    for item in values {
        let is_width_keyword = matches!(
            &item,
            Value::Keyword(keyword) if matches!(keyword.as_str(), "thin" | "medium" | "thick")
        );
        let is_border_style = matches!(
            &item,
            Value::Keyword(keyword)
                if matches!(
                    keyword.as_str(),
                    "none"
                        | "hidden"
                        | "dotted"
                        | "dashed"
                        | "solid"
                        | "double"
                        | "groove"
                        | "ridge"
                        | "inset"
                        | "outset"
                )
        );

        match item {
            Value::Length(_, _) if width.is_none() => width = Some(item),
            Value::Number(number) if number == 0.0 && width.is_none() => width = Some(item),
            Value::Keyword(_) if is_width_keyword && width.is_none() => width = Some(item),
            Value::Keyword(_) if is_border_style && style.is_none() => style = Some(item),
            Value::Color(_) | Value::Function { .. } | Value::Keyword(_) if color.is_none() => {
                color = Some(item)
            }
            _ => {}
        }
    }

    let mut declarations = Vec::new();
    if let Some(width) = width {
        declarations.push(Declaration {
            name: "border-width".to_string(),
            value: width.clone(),
            important,
        });
        declarations.extend(expand_border_axis_shorthand("border-width", width, important));
    }
    if let Some(style) = style {
        declarations.push(Declaration {
            name: "border-style".to_string(),
            value: style.clone(),
            important,
        });
        declarations.extend(expand_border_axis_shorthand("border-style", style, important));
    }
    if let Some(color) = color {
        declarations.push(Declaration {
            name: "border-color".to_string(),
            value: color.clone(),
            important,
        });
        declarations.extend(expand_border_axis_shorthand("border-color", color, important));
    }

    if declarations.is_empty() {
        declarations.push(Declaration {
            name: "border".to_string(),
            value: Value::List(Vec::new()),
            important,
        });
    }

    declarations
}

fn expand_border_side_shorthand(name: &str, value: Value, important: bool) -> Vec<Declaration> {
    let values = match value {
        Value::List(values) => values,
        single => vec![single],
    };

    let mut width = None;
    let mut style = None;
    let mut color = None;

    for item in values {
        let is_width_keyword = matches!(
            &item,
            Value::Keyword(keyword) if matches!(keyword.as_str(), "thin" | "medium" | "thick")
        );
        let is_border_style = matches!(
            &item,
            Value::Keyword(keyword)
                if matches!(
                    keyword.as_str(),
                    "none"
                        | "hidden"
                        | "dotted"
                        | "dashed"
                        | "solid"
                        | "double"
                        | "groove"
                        | "ridge"
                        | "inset"
                        | "outset"
                )
        );

        match item {
            Value::Length(_, _) | Value::Number(_) if width.is_none() => width = Some(item),
            Value::Keyword(_) if is_width_keyword && width.is_none() => width = Some(item),
            Value::Keyword(_) if is_border_style && style.is_none() => style = Some(item),
            Value::Color(_) | Value::Function { .. } | Value::Keyword(_) if color.is_none() => {
                color = Some(item)
            }
            _ => {}
        }
    }

    let mut declarations = Vec::new();
    if let Some(width) = width {
        declarations.push(Declaration {
            name: format!("{name}-width"),
            value: width,
            important,
        });
    }
    if let Some(style) = style {
        declarations.push(Declaration {
            name: format!("{name}-style"),
            value: style,
            important,
        });
    }
    if let Some(color) = color {
        declarations.push(Declaration {
            name: format!("{name}-color"),
            value: color,
            important,
        });
    }

    if declarations.is_empty() {
        declarations.push(Declaration {
            name: name.to_string(),
            value: Value::List(Vec::new()),
            important,
        });
    }

    declarations
}

fn expand_background_shorthand(value: Value, important: bool) -> Vec<Declaration> {
    let original = value.clone();
    let layers = match value {
        Value::CommaList(layers) => layers,
        single => vec![single],
    };
    let mut images = Vec::new();
    let mut positions_x = Vec::new();
    let mut positions_y = Vec::new();
    let mut sizes = Vec::new();
    let mut repeats = Vec::new();
    let mut attachments = Vec::new();
    let mut origins = Vec::new();
    let mut clips = Vec::new();
    let mut color = Value::Keyword("transparent".to_string());

    for (index, layer) in layers.iter().enumerate() {
        let is_last = index + 1 == layers.len();
        let Some(parsed) = parse_background_layer(layer, is_last) else {
            return vec![Declaration {
                name: "background".to_string(),
                value: original,
                important,
            }];
        };
        images.push(parsed.image);
        positions_x.push(parsed.position_x);
        positions_y.push(parsed.position_y);
        sizes.push(parsed.size);
        repeats.push(parsed.repeat);
        attachments.push(parsed.attachment);
        origins.push(parsed.origin);
        clips.push(parsed.clip);
        if let Some(layer_color) = parsed.color {
            color = layer_color;
        }
    }

    let layered = |values: Vec<Value>| {
        if values.len() == 1 {
            values.into_iter().next().expect("one background layer")
        } else {
            Value::CommaList(values)
        }
    };
    [
        ("background-image", layered(images)),
        ("background-position-x", layered(positions_x)),
        ("background-position-y", layered(positions_y)),
        ("background-size", layered(sizes)),
        ("background-repeat", layered(repeats)),
        ("background-attachment", layered(attachments)),
        ("background-origin", layered(origins)),
        ("background-clip", layered(clips)),
        ("background-color", color),
    ]
    .into_iter()
    .map(|(name, value)| Declaration {
        name: name.to_string(),
        value,
        important,
    })
    .collect()
}

struct BackgroundLayer {
    image: Value,
    position_x: Value,
    position_y: Value,
    size: Value,
    repeat: Value,
    attachment: Value,
    origin: Value,
    clip: Value,
    color: Option<Value>,
}

fn parse_background_layer(value: &Value, allow_color: bool) -> Option<BackgroundLayer> {
    let values = match value {
        Value::List(values) => values.as_slice(),
        single => std::slice::from_ref(single),
    };
    let slash = values
        .iter()
        .position(|value| matches!(value, Value::Keyword(keyword) if keyword == "/"));
    if values
        .iter()
        .filter(|value| matches!(value, Value::Keyword(keyword) if keyword == "/"))
        .count()
        > 1
    {
        return None;
    }
    let mut layer = BackgroundLayer {
        image: Value::Keyword("none".to_string()),
        position_x: Value::Percentage(0.0),
        position_y: Value::Percentage(0.0),
        size: Value::Keyword("auto".to_string()),
        repeat: Value::Keyword("repeat".to_string()),
        attachment: Value::Keyword("scroll".to_string()),
        origin: Value::Keyword("padding-box".to_string()),
        clip: Value::Keyword("border-box".to_string()),
        color: None,
    };
    let mut position = Vec::new();
    let mut boxes = Vec::new();
    let mut saw_image = false;
    let mut repeat_values = Vec::new();
    let mut saw_attachment = false;
    let size_range = if let Some(slash) = slash {
        let mut end = slash + 1;
        while end < values.len() && end <= slash + 2 && is_background_size_value(&values[end]) {
            end += 1;
        }
        if end == slash + 1 {
            return None;
        }
        let size_values = &values[slash + 1..end];
        layer.size = if size_values.len() == 1 {
            size_values[0].clone()
        } else {
            Value::List(size_values.to_vec())
        };
        Some(slash + 1..end)
    } else {
        None
    };

    for (index, item) in values.iter().enumerate() {
        if Some(index) == slash {
            continue;
        }
        if size_range.as_ref().is_some_and(|range| range.contains(&index)) {
            continue;
        }
        match item {
            Value::Function { name, .. } if is_background_image_function(name) && !saw_image => {
                layer.image = item.clone();
                saw_image = true;
            }
            Value::Keyword(keyword)
                if !saw_image
                    && (keyword.eq_ignore_ascii_case("none")
                        || keyword.to_ascii_lowercase().starts_with("url(")) =>
            {
                layer.image = item.clone();
                saw_image = true;
            }
            Value::Keyword(keyword)
                if matches!(
                    keyword.to_ascii_lowercase().as_str(),
                    "repeat" | "no-repeat" | "repeat-x" | "repeat-y"
                ) =>
            {
                if !repeat_values.is_empty()
                    && (matches!(keyword.to_ascii_lowercase().as_str(), "repeat-x" | "repeat-y")
                        || repeat_values.iter().any(|value| {
                            matches!(value, Value::Keyword(previous) if matches!(previous.to_ascii_lowercase().as_str(), "repeat-x" | "repeat-y"))
                        }))
                {
                    return None;
                }
                repeat_values.push(item.clone());
                if repeat_values.len() > 2 {
                    return None;
                }
            }
            Value::Keyword(keyword)
                if matches!(keyword.to_ascii_lowercase().as_str(), "scroll" | "fixed" | "local")
                    && !saw_attachment =>
            {
                layer.attachment = item.clone();
                saw_attachment = true;
            }
            Value::Keyword(keyword)
                if matches!(
                    keyword.to_ascii_lowercase().as_str(),
                    "border-box" | "padding-box" | "content-box"
                ) => boxes.push(item.clone()),
            Value::Color(_) => {
                if !allow_color || layer.color.is_some() {
                    return None;
                }
                layer.color = Some(item.clone());
            }
            Value::Function { name, .. } if is_color_function(name) => {
                if !allow_color || layer.color.is_some() {
                    return None;
                }
                layer.color = Some(item.clone());
            }
            Value::Keyword(keyword) if is_background_color_keyword(keyword) => {
                if !allow_color || layer.color.is_some() {
                    return None;
                }
                layer.color = Some(item.clone());
            }
            Value::Keyword(keyword)
                if matches!(
                    keyword.to_ascii_lowercase().as_str(),
                    "left" | "right" | "top" | "bottom" | "center"
                ) => position.push(item.clone()),
            Value::Length(_, _) | Value::Percentage(_) => position.push(item.clone()),
            Value::Number(number) if *number == 0.0 => position.push(item.clone()),
            _ => return None,
        }
    }
    if position.len() > 2 || boxes.len() > 2 {
        return None;
    }
    if repeat_values.len() == 1 {
        layer.repeat = repeat_values.remove(0);
    } else if repeat_values.len() == 2 {
        layer.repeat = Value::List(repeat_values);
    }
    if !position.is_empty() {
        let (x, y) = normalize_background_position(&position)?;
        layer.position_x = x;
        layer.position_y = y;
    }
    if let Some(origin) = boxes.first() {
        layer.origin = origin.clone();
        layer.clip = origin.clone();
    }
    if let Some(clip) = boxes.get(1) {
        layer.clip = clip.clone();
    }
    Some(layer)
}

fn is_background_size_value(value: &Value) -> bool {
    match value {
        Value::Length(_, _) | Value::Percentage(_) => true,
        Value::Number(number) => *number == 0.0,
        Value::Function { name, .. } => {
            matches!(name.to_ascii_lowercase().as_str(), "calc" | "clamp")
        }
        Value::Keyword(keyword) => {
            matches!(keyword.to_ascii_lowercase().as_str(), "auto" | "cover" | "contain")
        }
        _ => false,
    }
}

fn is_background_image_function(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "linear-gradient"
            | "radial-gradient"
            | "conic-gradient"
            | "repeating-linear-gradient"
            | "repeating-radial-gradient"
            | "repeating-conic-gradient"
    )
}

fn is_color_function(name: &str) -> bool {
    matches!(name.to_ascii_lowercase().as_str(), "rgb" | "rgba" | "hsl" | "hsla")
}

fn expand_mask_shorthand(value: Value, important: bool) -> Vec<Declaration> {
    let values = match value {
        Value::List(values) => values,
        single => vec![single],
    };
    let slash = values
        .iter()
        .position(|value| matches!(value, Value::Keyword(keyword) if keyword == "/"));
    let before_size = slash.map_or(values.as_slice(), |index| &values[..index]);
    let after_size = slash.map_or(&[][..], |index| &values[index + 1..]);

    let mut image = None;
    let mut unsupported_image = false;
    let mut position = Vec::new();
    let mut size = Vec::new();
    let mut repeat = None;

    for item in before_size {
        match item {
            Value::Keyword(keyword) if keyword.to_ascii_lowercase().starts_with("url(") => {
                if image.is_none() {
                    image = Some(item.clone());
                }
            }
            Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("none") => {
                image = Some(item.clone());
            }
            Value::Keyword(keyword)
                if keyword.eq_ignore_ascii_case("repeat")
                    || keyword.eq_ignore_ascii_case("no-repeat") =>
            {
                repeat = Some(item.clone());
            }
            Value::Function { .. } => unsupported_image = true,
            item if is_mask_position_value(item) => position.push(item.clone()),
            _ => {}
        }
    }
    for item in after_size {
        match item {
            Value::Keyword(keyword)
                if keyword.eq_ignore_ascii_case("repeat")
                    || keyword.eq_ignore_ascii_case("no-repeat") =>
            {
                repeat = Some(item.clone());
            }
            item if is_mask_size_value(item) => size.push(item.clone()),
            _ => {}
        }
    }

    let mut declarations = Vec::new();
    // This implementation supports only URL masks. Gradients and other image
    // functions deliberately compute to `none`, i.e. no mask is applied.
    if unsupported_image && image.is_none() {
        image = Some(Value::Keyword("none".to_string()));
    }
    if let Some(value) = image {
        declarations.push(Declaration {
            name: "mask-image".to_string(),
            value,
            important,
        });
    }
    declarations.extend(expand_mask_position_values(&position, important));
    if !size.is_empty() {
        declarations.push(Declaration {
            name: "mask-size".to_string(),
            value: collapse_mask_values(&size),
            important,
        });
    }
    if let Some(value) = repeat {
        declarations.push(Declaration {
            name: "mask-repeat".to_string(),
            value,
            important,
        });
    }
    declarations
}

fn expand_mask_position_shorthand(value: Value, important: bool) -> Vec<Declaration> {
    let values = match value {
        Value::List(values) => values,
        single => vec![single],
    };
    expand_mask_position_values(&values, important)
}

fn expand_mask_position_values(values: &[Value], important: bool) -> Vec<Declaration> {
    let (x, y) = match values {
        [] => return Vec::new(),
        [value] => {
            let keyword = value_keyword(value);
            if matches!(keyword, Some("top" | "bottom")) {
                (Value::Keyword("center".to_string()), value.clone())
            } else {
                (value.clone(), Value::Keyword("center".to_string()))
            }
        }
        [first, second, ..] => {
            let first_keyword = value_keyword(first);
            let second_keyword = value_keyword(second);
            if matches!(first_keyword, Some("top" | "bottom"))
                || matches!(second_keyword, Some("left" | "right"))
            {
                (second.clone(), first.clone())
            } else {
                (first.clone(), second.clone())
            }
        }
    };
    vec![
        Declaration { name: "mask-position-x".to_string(), value: x, important },
        Declaration { name: "mask-position-y".to_string(), value: y, important },
    ]
}

fn value_keyword(value: &Value) -> Option<&str> {
    match value {
        Value::Keyword(keyword) => Some(keyword.as_str()),
        _ => None,
    }
}

fn is_mask_position_value(value: &Value) -> bool {
    matches!(value, Value::Length(..) | Value::Percentage(_) | Value::Number(_))
        || matches!(value_keyword(value), Some("left" | "right" | "top" | "bottom" | "center"))
}

fn is_mask_size_value(value: &Value) -> bool {
    matches!(value, Value::Length(..) | Value::Percentage(_) | Value::Number(_))
        || matches!(value_keyword(value), Some("auto" | "contain" | "cover"))
}

fn collapse_mask_values(values: &[Value]) -> Value {
    match values {
        [value] => value.clone(),
        values => Value::List(values.to_vec()),
    }
}

fn expand_font_shorthand(value: Value, important: bool) -> Vec<Declaration> {
    let values = match value {
        Value::List(values) => values,
        single => vec![single],
    };

    let mut declarations = Vec::new();
    for item in &values {
        match item {
            Value::Length(_, unit) if unit == "px" || unit == "em" => {
                declarations.push(Declaration {
                    name: "font-size".to_string(),
                    value: item.clone(),
                    important,
                })
            }
            Value::Percentage(_) => declarations.push(Declaration {
                name: "font-size".to_string(),
                value: item.clone(),
                important,
            }),
            Value::Keyword(keyword) => {
                if let Some((font_size, line_height)) = keyword.split_once('/') {
                    if let Some(size) = parse_font_shorthand_length(font_size.trim()) {
                        declarations.push(Declaration {
                            name: "font-size".to_string(),
                            value: size,
                            important,
                        });
                    }
                    if let Some(height) = parse_font_shorthand_length(line_height.trim()) {
                        declarations.push(Declaration {
                            name: "line-height".to_string(),
                            value: height,
                            important,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    if declarations.is_empty() {
        vec![Declaration {
            name: "font".to_string(),
            value: Value::List(values),
            important,
        }]
    } else {
        declarations
    }
}

fn parse_font_shorthand_length(value: &str) -> Option<Value> {
    if let Some(unit) = value.strip_suffix("px") {
        return unit
            .trim()
            .parse()
            .ok()
            .map(|number| Value::Length(number, "px".to_string()));
    }
    if let Some(unit) = value.strip_suffix("em") {
        return unit
            .trim()
            .parse()
            .ok()
            .map(|number| Value::Length(number, "em".to_string()));
    }
    if let Some(unit) = value.strip_suffix('%') {
        return unit.trim().parse().ok().map(Value::Percentage);
    }
    None
}

fn expand_overflow_shorthand(value: Value, important: bool) -> Vec<Declaration> {
    let values = match value {
        Value::List(values) => values,
        single => vec![single],
    };

    match values.as_slice() {
        [a] => vec![
            Declaration {
                name: "overflow-x".to_string(),
                value: a.clone(),
                important,
            },
            Declaration {
                name: "overflow-y".to_string(),
                value: a.clone(),
                important,
            },
        ],
        [x, y] => vec![
            Declaration {
                name: "overflow-x".to_string(),
                value: x.clone(),
                important,
            },
            Declaration {
                name: "overflow-y".to_string(),
                value: y.clone(),
                important,
            },
        ],
        _ => vec![Declaration {
            name: "overflow".to_string(),
            value: Value::List(values),
            important,
        }],
    }
}

fn expand_flex_shorthand(value: Value, important: bool) -> Vec<Declaration> {
    let values = match value {
        Value::List(values) => values,
        single => vec![single],
    };

    // CSS-wide keywords: propagate to all three longhands
    if let [Value::Keyword(kw)] = values.as_slice() {
        let lower = kw.to_ascii_lowercase();
        if matches!(lower.as_str(), "inherit" | "initial" | "unset" | "revert") {
            return vec![
                Declaration {
                    name: "flex-grow".to_string(),
                    value: Value::Keyword(lower.clone()),
                    important,
                },
                Declaration {
                    name: "flex-shrink".to_string(),
                    value: Value::Keyword(lower.clone()),
                    important,
                },
                Declaration {
                    name: "flex-basis".to_string(),
                    value: Value::Keyword(lower),
                    important,
                },
            ];
        }
    }

    // flex: none → 0 0 auto
    if let [Value::Keyword(kw)] = values.as_slice() {
        if kw == "none" {
            return vec![
                Declaration {
                    name: "flex-grow".to_string(),
                    value: Value::Number(0.0),
                    important,
                },
                Declaration {
                    name: "flex-shrink".to_string(),
                    value: Value::Number(0.0),
                    important,
                },
                Declaration {
                    name: "flex-basis".to_string(),
                    value: Value::Keyword("auto".to_string()),
                    important,
                },
            ];
        }
        // flex: auto → 1 1 auto
        if kw == "auto" {
            return vec![
                Declaration {
                    name: "flex-grow".to_string(),
                    value: Value::Number(1.0),
                    important,
                },
                Declaration {
                    name: "flex-shrink".to_string(),
                    value: Value::Number(1.0),
                    important,
                },
                Declaration {
                    name: "flex-basis".to_string(),
                    value: Value::Keyword("auto".to_string()),
                    important,
                },
            ];
        }
    }

    // flex: <grow> → grow shrink=1 basis=0  (単独の数値)
    if let [Value::Number(grow)] = values.as_slice() {
        return vec![
            Declaration {
                name: "flex-grow".to_string(),
                value: Value::Number(*grow),
                important,
            },
            Declaration {
                name: "flex-shrink".to_string(),
                value: Value::Number(1.0),
                important,
            },
            Declaration {
                name: "flex-basis".to_string(),
                value: Value::Number(0.0),
                important,
            },
        ];
    }

    // flex: <basis> → grow=1 shrink=1 basis  (単独の length/percentage)
    if let [basis] = values.as_slice()
        && matches!(basis, Value::Length(_, _) | Value::Percentage(_)) {
            return vec![
                Declaration {
                    name: "flex-grow".to_string(),
                    value: Value::Number(1.0),
                    important,
                },
                Declaration {
                    name: "flex-shrink".to_string(),
                    value: Value::Number(1.0),
                    important,
                },
                Declaration {
                    name: "flex-basis".to_string(),
                    value: basis.clone(),
                    important,
                },
            ];
        }

    // flex: <grow> <shrink> <basis>
    if let [grow, shrink, basis] = values.as_slice()
        && matches!(grow, Value::Number(_)) && matches!(shrink, Value::Number(_)) {
            return vec![
                Declaration {
                    name: "flex-grow".to_string(),
                    value: grow.clone(),
                    important,
                },
                Declaration {
                    name: "flex-shrink".to_string(),
                    value: shrink.clone(),
                    important,
                },
                Declaration {
                    name: "flex-basis".to_string(),
                    value: basis.clone(),
                    important,
                },
            ];
        }

    // flex: <grow> <basis>  (数値 + length/percentage)
    if let [grow, basis] = values.as_slice()
        && matches!(grow, Value::Number(_))
            && matches!(basis, Value::Length(_, _) | Value::Percentage(_))
        {
            return vec![
                Declaration {
                    name: "flex-grow".to_string(),
                    value: grow.clone(),
                    important,
                },
                Declaration {
                    name: "flex-shrink".to_string(),
                    value: Value::Number(1.0),
                    important,
                },
                Declaration {
                    name: "flex-basis".to_string(),
                    value: basis.clone(),
                    important,
                },
            ];
        }

    // flex: <grow> <shrink>  (2値でどちらも数値)
    if let [grow, shrink] = values.as_slice()
        && matches!(grow, Value::Number(_)) && matches!(shrink, Value::Number(_)) {
            return vec![
                Declaration {
                    name: "flex-grow".to_string(),
                    value: grow.clone(),
                    important,
                },
                Declaration {
                    name: "flex-shrink".to_string(),
                    value: shrink.clone(),
                    important,
                },
                Declaration {
                    name: "flex-basis".to_string(),
                    value: Value::Number(0.0),
                    important,
                },
            ];
        }

    // フォールバック: そのまま保持（単一値はListで包まない）
    let fallback_value = if values.len() == 1 {
        values.into_iter().next().unwrap()
    } else {
        Value::List(values)
    };
    vec![Declaration {
        name: "flex".to_string(),
        value: fallback_value,
        important,
    }]
}

/// Expand `text-decoration` shorthand into its longhands.
///
/// CSS spec: `text-decoration` is a shorthand for `text-decoration-line`,
/// `text-decoration-style`, and `text-decoration-color`. The values can appear
/// in any order. Unknown values are left as-is.
fn expand_text_decoration_shorthand(value: Value, important: bool) -> Vec<Declaration> {
    let values = match value {
        Value::List(values) => values,
        single => vec![single],
    };

    // CSS-wide keywords: propagate to all three longhands
    if let [Value::Keyword(kw)] = values.as_slice() {
        let lower = kw.to_ascii_lowercase();
        if matches!(lower.as_str(), "inherit" | "initial" | "unset" | "revert") {
            return vec![
                Declaration {
                    name: "text-decoration-line".to_string(),
                    value: Value::Keyword(lower.clone()),
                    important,
                },
                Declaration {
                    name: "text-decoration-style".to_string(),
                    value: Value::Keyword(lower.clone()),
                    important,
                },
                Declaration {
                    name: "text-decoration-color".to_string(),
                    value: Value::Keyword(lower),
                    important,
                },
            ];
        }
    }

    let mut line_parts: Vec<String> = Vec::new();
    let mut style: Option<Value> = None;
    let mut color: Option<Value> = None;

    for item in &values {
        match item {
            Value::Keyword(kw) => {
                let lower = kw.to_ascii_lowercase();
                match lower.as_str() {
                    "none" | "underline" | "overline" | "line-through" | "blink" => {
                        line_parts.push(lower);
                    }
                    "solid" | "dashed" | "dotted" | "double" | "wavy" => {
                        if style.is_none() {
                            style = Some(Value::Keyword(lower));
                        }
                    }
                    _ if crate::css::style::is_color_keyword(&lower)
                        && color.is_none() => {
                            color = Some(Value::Keyword(lower));
                        }
                    _ => {}
                }
            }
            Value::Color(_) | Value::Function { .. }
                if color.is_none() => {
                    color = Some(item.clone());
                }
            _ => {}
        }
    }

    let mut decls = Vec::new();
    if !line_parts.is_empty() {
        let line_value = line_parts.join(" ");
        decls.push(Declaration {
            name: "text-decoration-line".to_string(),
            value: Value::Keyword(line_value),
            important,
        });
    }
    if let Some(v) = style {
        decls.push(Declaration {
            name: "text-decoration-style".to_string(),
            value: v,
            important,
        });
    }
    if let Some(v) = color {
        decls.push(Declaration {
            name: "text-decoration-color".to_string(),
            value: v,
            important,
        });
    }

    // Fallback: preserve original if nothing matched
    if decls.is_empty() {
        let fallback_value = if values.len() == 1 {
            values.into_iter().next().unwrap()
        } else {
            Value::List(values)
        };
        return vec![Declaration {
            name: "text-decoration".to_string(),
            value: fallback_value,
            important,
        }];
    }

    decls
}

/// box-shadow 宣言を単一の Declaration に変換する。
/// 値はそのまま保持し、paint 側でパースする。
fn expand_box_shadow_shorthand(value: Value, important: bool) -> Vec<Declaration> {
    vec![Declaration {
        name: "box-shadow".to_string(),
        value,
        important,
    }]
}

/// Expands `list-style` shorthand into `list-style-type`, `list-style-position`,
/// and `list-style-image` longhands.
///
/// Syntax: `list-style: <type> || <position> || <image>`
/// where type is a keyword (disc, circle, square, decimal, none, …),
/// position is `inside` or `outside`, and image is `none` or a `url(…)`.
fn expand_list_style_shorthand(value: Value, important: bool) -> Vec<Declaration> {
    const POSITION_KEYWORDS: &[&str] = &["inside", "outside"];
    const TYPE_KEYWORDS: &[&str] = &[
        "disc", "circle", "square", "decimal", "lower-roman", "upper-roman",
        "lower-alpha", "upper-alpha", "lower-latin", "upper-latin", "none",
    ];

    let values: Vec<Value> = match value {
        Value::List(items) => items,
        single => vec![single],
    };

    let mut list_style_type: Option<Value> = None;
    let mut list_style_position: Option<Value> = None;
    let mut list_style_image: Option<Value> = None;

    // Check for bare `none` — sets both type and image to none
    if values.len() == 1
        && let Value::Keyword(kw) = &values[0]
            && kw.eq_ignore_ascii_case("none") {
                return vec![
                    Declaration {
                        name: "list-style-type".to_string(),
                        value: Value::Keyword("none".to_string()),
                        important,
                    },
                    Declaration {
                        name: "list-style-position".to_string(),
                        value: Value::Keyword("outside".to_string()),
                        important,
                    },
                    Declaration {
                        name: "list-style-image".to_string(),
                        value: Value::Keyword("none".to_string()),
                        important,
                    },
                ];
            }

    // First pass: detect if a non-none type keyword is present (order-independent)
    let has_explicit_type = values.iter().any(|v| {
        matches!(v, Value::Keyword(kw) if {
            let lc = kw.to_ascii_lowercase();
            lc != "none" && !POSITION_KEYWORDS.contains(&lc.as_str()) && TYPE_KEYWORDS.contains(&lc.as_str())
        })
    });

    for val in values {
        match &val {
            Value::Keyword(kw) => {
                let lc = kw.to_ascii_lowercase();
                if POSITION_KEYWORDS.contains(&lc.as_str()) {
                    list_style_position.get_or_insert(val);
                } else if lc == "none" {
                    // `none` can appear as list-style-type OR list-style-image.
                    // If a non-none type keyword is present elsewhere, treat `none` as image=none.
                    if has_explicit_type || list_style_type.is_some() {
                        list_style_image.get_or_insert(val);
                    } else {
                        list_style_type.get_or_insert(val);
                    }
                } else {
                    list_style_type.get_or_insert(val);
                }
            }
            Value::Function { name, .. } if name.eq_ignore_ascii_case("url") => {
                list_style_image.get_or_insert(val);
            }
            _ => {}
        }
    }

    // CSS shorthand rule: always emit all three longhands, using initial values
    // for any subproperty that was not explicitly present in the shorthand.
    // Initial values: list-style-type = disc, list-style-position = outside, list-style-image = none
    vec![Declaration {
        name: "list-style-type".to_string(),
        value: list_style_type.unwrap_or(Value::Keyword("disc".to_string())),
        important,
    }, Declaration {
        name: "list-style-position".to_string(),
        value: list_style_position.unwrap_or(Value::Keyword("outside".to_string())),
        important,
    }, Declaration {
        name: "list-style-image".to_string(),
        value: list_style_image.unwrap_or(Value::Keyword("none".to_string())),
        important,
    }]
}

/// Expands `flex-flow` shorthand into `flex-direction` and `flex-wrap` longhands.
///
/// Syntax: `flex-flow: <flex-direction> || <flex-wrap>`
///
/// direction keywords: `row`, `row-reverse`, `column`, `column-reverse`
/// wrap keywords: `nowrap`, `wrap`, `wrap-reverse`
///
/// Any omitted subproperty receives its initial value:
/// - `flex-direction` initial: `row`
/// - `flex-wrap` initial: `nowrap`
fn expand_flex_flow_shorthand(value: Value, important: bool) -> Vec<Declaration> {
    const DIRECTION_KEYWORDS: &[&str] = &["row", "row-reverse", "column", "column-reverse"];
    const WRAP_KEYWORDS: &[&str] = &["nowrap", "wrap", "wrap-reverse"];

    let values = match value {
        Value::List(values) => values,
        single => vec![single],
    };

    // CSS-wide keywords: propagate to both longhands
    if let [Value::Keyword(kw)] = values.as_slice() {
        let lower = kw.to_ascii_lowercase();
        if matches!(lower.as_str(), "inherit" | "initial" | "unset" | "revert") {
            return vec![
                Declaration {
                    name: "flex-direction".to_string(),
                    value: Value::Keyword(lower.clone()),
                    important,
                },
                Declaration {
                    name: "flex-wrap".to_string(),
                    value: Value::Keyword(lower),
                    important,
                },
            ];
        }
    }

    let mut direction: Option<Value> = None;
    let mut wrap: Option<Value> = None;

    for item in values {
        if let Value::Keyword(kw) = &item {
            let lower = kw.to_ascii_lowercase();
            if DIRECTION_KEYWORDS.contains(&lower.as_str()) && direction.is_none() {
                direction = Some(Value::Keyword(lower));
            } else if WRAP_KEYWORDS.contains(&lower.as_str()) && wrap.is_none() {
                wrap = Some(Value::Keyword(lower));
            }
        }
    }

    vec![
        Declaration {
            name: "flex-direction".to_string(),
            value: direction.unwrap_or(Value::Keyword("row".to_string())),
            important,
        },
        Declaration {
            name: "flex-wrap".to_string(),
            value: wrap.unwrap_or(Value::Keyword("nowrap".to_string())),
            important,
        },
    ]
}

fn is_background_color_keyword(keyword: &str) -> bool {
    matches!(
        keyword,
        "transparent"
            | "black"
            | "white"
            | "red"
            | "green"
            | "blue"
            | "gray"
            | "grey"
            | "navy"
            | "yellow"
    )
}

/// Expand `animation` shorthand into longhands.
///
/// Simplified parser that extracts `animation-name` and `animation-fill-mode`
/// from the shorthand value. Timing functions, delays, and iteration counts
/// are preserved as the original `animation` declaration for future use.
fn expand_animation_shorthand(value: Value, important: bool) -> Vec<Declaration> {
    let values = match &value {
        Value::List(vs) => vs.clone(),
        other => vec![other.clone()],
    };

    let mut name: Option<String> = None;
    let mut fill_mode: Option<String> = None;
    let mut duration: Option<Value> = None;
    let mut iteration_count: Option<String> = None;

    for item in &values {
        if let Value::Keyword(kw) = item {
            let lower = kw.to_ascii_lowercase();
            match lower.as_str() {
                "none" => {
                    // "none" is primarily animation-name: none (clears animation).
                    if name.is_none() {
                        name = Some("none".to_string());
                    }
                }
                "forwards" | "backwards" | "both" => {
                    if fill_mode.is_none() {
                        fill_mode = Some(lower);
                    }
                }
                "normal" | "reverse" | "alternate" | "alternate-reverse" => {
                    // animation-direction — skip for now
                }
                "infinite" => {
                    iteration_count = Some(lower);
                }
                "ease" | "ease-in" | "ease-out" | "ease-in-out" | "linear" | "step-start"
                | "step-end" => {
                    // animation-timing-function — skip for now
                }
                "running" | "paused" => {
                    // animation-play-state — skip for now
                }
                _ => {
                    if name.is_none() {
                        name = Some(kw.clone());
                    }
                }
            }
        } else if let Value::Length(_, unit) = item
            && (unit == "s" || unit == "ms") && duration.is_none() {
                duration = Some(item.clone());
            }
    }

    let mut decls = Vec::new();
    if let Some(name) = name {
        decls.push(Declaration {
            name: "animation-name".to_string(),
            value: Value::Keyword(name),
            important,
        });
    }
    if let Some(fill_mode) = fill_mode {
        decls.push(Declaration {
            name: "animation-fill-mode".to_string(),
            value: Value::Keyword(fill_mode),
            important,
        });
    }
    if let Some(duration) = duration {
        decls.push(Declaration {
            name: "animation-duration".to_string(),
            value: duration,
            important,
        });
    }
    if let Some(iteration_count) = iteration_count {
        decls.push(Declaration {
            name: "animation-iteration-count".to_string(),
            value: Value::Keyword(iteration_count),
            important,
        });
    }

    // Keep original animation declaration as well for properties we don't expand
    decls.push(Declaration {
        name: "animation".to_string(),
        value,
        important,
    });

    decls
}

/// Expand `outline` shorthand into `outline-style`, `outline-width`, `outline-color`.
fn expand_outline_shorthand(value: Value, important: bool) -> Vec<Declaration> {
    let values = match &value {
        Value::List(vs) => vs.clone(),
        other => vec![other.clone()],
    };

    // CSS-wide keywords apply to all longhands at once.
    if values.len() == 1
        && let Value::Keyword(kw) = &values[0] {
            let lower = kw.to_ascii_lowercase();
            if matches!(lower.as_str(), "inherit" | "initial" | "unset" | "revert") {
                return vec![
                    Declaration { name: "outline-style".to_string(), value: Value::Keyword(lower.clone()), important },
                    Declaration { name: "outline-width".to_string(), value: Value::Keyword(lower.clone()), important },
                    Declaration { name: "outline-color".to_string(), value: Value::Keyword(lower), important },
                ];
            }
        }

    let mut style = None;
    let mut width = None;
    let mut color = None;

    for item in &values {
        if let Value::Keyword(kw) = item {
            let lower = kw.to_ascii_lowercase();
            match lower.as_str() {
                "none" | "solid" | "dashed" | "dotted" | "double" | "groove" | "ridge"
                | "inset" | "outset" | "auto" => {
                    if style.is_none() {
                        style = Some(Value::Keyword(lower));
                    }
                }
                "thin" | "medium" | "thick" => {
                    if width.is_none() {
                        width = Some(item.clone());
                    }
                }
                _ if is_background_color_keyword(&lower)
                    && color.is_none() => {
                        color = Some(item.clone());
                    }
                _ => {
                    // Unknown keyword — skip rather than mis-classify as color
                }
            }
        } else if let Value::Length(_, _) = item {
            if width.is_none() {
                width = Some(item.clone());
            }
        } else if let Value::Number(v) = item {
            if *v == 0.0 && width.is_none() {
                width = Some(item.clone());
            }
        } else if let Value::Function { .. } = item {
            // Color functions like rgb(), hsl()
            if color.is_none() {
                color = Some(item.clone());
            }
        }
    }

    // Always emit all three longhands, using initial values for omitted ones.
    // This ensures `outline: none` resets width and color to their defaults.
    vec![
        Declaration {
            name: "outline-style".to_string(),
            value: style.unwrap_or_else(|| Value::Keyword("none".to_string())),
            important,
        },
        Declaration {
            name: "outline-width".to_string(),
            value: width.unwrap_or_else(|| Value::Keyword("medium".to_string())),
            important,
        },
        Declaration {
            name: "outline-color".to_string(),
            value: color.unwrap_or_else(|| Value::Keyword("currentcolor".to_string())),
            important,
        },
    ]
}
