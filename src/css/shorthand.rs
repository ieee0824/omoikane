//! CSS shorthand property expansion.

use super::{Declaration, Value};

pub(super) fn expand_shorthand(name: &str, value: Value, important: bool) -> Vec<Declaration> {
    match name {
        "margin" | "padding" => expand_box_shorthand(name, value, important),
        "border-width" | "border-style" | "border-color" => {
            expand_border_axis_shorthand(name, value, important)
        }
        "border" => expand_border_shorthand(value, important),
        "border-top" | "border-right" | "border-bottom" | "border-left" => {
            expand_border_side_shorthand(name, value, important)
        }
        "background" => expand_background_shorthand(value, important),
        "font" => expand_font_shorthand(value, important),
        "overflow" => expand_overflow_shorthand(value, important),
        "flex" => expand_flex_shorthand(value, important),
        "text-decoration" => expand_text_decoration_shorthand(value, important),
        "border-radius" => expand_border_radius_shorthand(value, important),
        "box-shadow" => expand_box_shadow_shorthand(value, important),
        "list-style" => expand_list_style_shorthand(value, important),
        "flex-flow" => expand_flex_flow_shorthand(value, important),
        "animation" => expand_animation_shorthand(value, important),
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
            value: width,
            important,
        });
    }
    if let Some(style) = style {
        declarations.push(Declaration {
            name: "border-style".to_string(),
            value: style,
            important,
        });
    }
    if let Some(color) = color {
        declarations.push(Declaration {
            name: "border-color".to_string(),
            value: color,
            important,
        });
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
    let values = match value {
        Value::List(values) => values,
        single => vec![single],
    };

    let mut declarations = Vec::new();
    let mut position_values = Vec::new();
    for item in &values {
        match item {
            Value::Function { name, .. } if name.eq_ignore_ascii_case("url") => {
                declarations.push(Declaration {
                    name: "background-image".to_string(),
                    value: item.clone(),
                    important,
                })
            }
            Value::Keyword(keyword)
                if keyword.eq_ignore_ascii_case("repeat")
                    || keyword.eq_ignore_ascii_case("no-repeat") =>
            {
                declarations.push(Declaration {
                    name: "background-repeat".to_string(),
                    value: Value::Keyword(keyword.to_string()),
                    important,
                });
            }
            Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("fixed") => {
                declarations.push(Declaration {
                    name: "background-attachment".to_string(),
                    value: Value::Keyword(keyword.to_string()),
                    important,
                });
            }
            Value::Function { name, .. }
                if name.eq_ignore_ascii_case("linear-gradient")
                    || name.eq_ignore_ascii_case("radial-gradient")
                    || name.eq_ignore_ascii_case("conic-gradient")
                    || name.eq_ignore_ascii_case("repeating-linear-gradient")
                    || name.eq_ignore_ascii_case("repeating-radial-gradient") =>
            {
                declarations.push(Declaration {
                    name: "background-image".to_string(),
                    value: item.clone(),
                    important,
                })
            }
            Value::Color(_) | Value::Function { .. } => declarations.push(Declaration {
                name: "background-color".to_string(),
                value: item.clone(),
                important,
            }),
            Value::Keyword(keyword) if is_background_color_keyword(&keyword) => {
                declarations.push(Declaration {
                    name: "background-color".to_string(),
                    value: Value::Keyword(keyword.to_string()),
                    important,
                });
            }
            Value::Keyword(keyword) if keyword.starts_with("url(") => {
                declarations.push(Declaration {
                    name: "background-image".to_string(),
                    value: Value::Keyword(keyword.to_string()),
                    important,
                });
            }
            Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("none") => {
                declarations.push(Declaration {
                    name: "background-image".to_string(),
                    value: Value::Keyword(keyword.to_string()),
                    important,
                });
                declarations.push(Declaration {
                    name: "background-color".to_string(),
                    value: Value::Keyword("transparent".to_string()),
                    important,
                });
            }
            Value::Length(_, unit) if unit == "px" || unit == "em" => {
                position_values.push(item.clone());
            }
            Value::Number(_) => {
                position_values.push(item.clone());
            }
            _ => {
                // Unknown value in background shorthand → reject the entire shorthand
                return vec![Declaration {
                    name: "background".to_string(),
                    value: Value::List(values),
                    important,
                }];
            }
        }
    }

    if let Some(first) = position_values.first() {
        declarations.push(Declaration {
            name: "background-position-x".to_string(),
            value: first.clone(),
            important,
        });
    }
    if let Some(second) = position_values.get(1) {
        declarations.push(Declaration {
            name: "background-position-y".to_string(),
            value: second.clone(),
            important,
        });
    }

    // Reject shorthand if multiple background-color values were found (e.g. "red pink")
    let color_count = declarations
        .iter()
        .filter(|d| d.name == "background-color")
        .count();
    if color_count > 1 {
        return vec![Declaration {
            name: "background".to_string(),
            value: Value::List(values),
            important,
        }];
    }

    if declarations.is_empty() {
        vec![Declaration {
            name: "background".to_string(),
            value: Value::List(values),
            important,
        }]
    } else {
        declarations
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
    if let [basis] = values.as_slice() {
        if matches!(basis, Value::Length(_, _) | Value::Percentage(_)) {
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
    }

    // flex: <grow> <shrink> <basis>
    if let [grow, shrink, basis] = values.as_slice() {
        if matches!(grow, Value::Number(_)) && matches!(shrink, Value::Number(_)) {
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
    }

    // flex: <grow> <basis>  (数値 + length/percentage)
    if let [grow, basis] = values.as_slice() {
        if matches!(grow, Value::Number(_))
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
    }

    // flex: <grow> <shrink>  (2値でどちらも数値)
    if let [grow, shrink] = values.as_slice() {
        if matches!(grow, Value::Number(_)) && matches!(shrink, Value::Number(_)) {
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
                    _ if crate::css::style::is_color_keyword(&lower) => {
                        if color.is_none() {
                            color = Some(Value::Keyword(lower));
                        }
                    }
                    _ => {}
                }
            }
            Value::Color(_) | Value::Function { .. } => {
                if color.is_none() {
                    color = Some(item.clone());
                }
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
    if values.len() == 1 {
        if let Value::Keyword(kw) = &values[0] {
            if kw.eq_ignore_ascii_case("none") {
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
        }
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
                } else if TYPE_KEYWORDS.contains(&lc.as_str()) {
                    list_style_type.get_or_insert(val);
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
    let mut decls = Vec::new();

    decls.push(Declaration {
        name: "list-style-type".to_string(),
        value: list_style_type.unwrap_or(Value::Keyword("disc".to_string())),
        important,
    });
    decls.push(Declaration {
        name: "list-style-position".to_string(),
        value: list_style_position.unwrap_or(Value::Keyword("outside".to_string())),
        important,
    });
    decls.push(Declaration {
        name: "list-style-image".to_string(),
        value: list_style_image.unwrap_or(Value::Keyword("none".to_string())),
        important,
    });

    decls
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
                    // animation-iteration-count — skip for now
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
        } else if let Value::Length(_, unit) = item {
            if (unit == "s" || unit == "ms") && duration.is_none() {
                duration = Some(item.clone());
            }
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

    // Keep original animation declaration as well for properties we don't expand
    decls.push(Declaration {
        name: "animation".to_string(),
        value,
        important,
    });

    decls
}
