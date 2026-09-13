//! Font shorthand parsing before slash/comma token boundaries are lost.
use super::tokenizer::render_tokens;
use super::{CssParseError, CssToken, Declaration, Value};

const LONGHANDS: [&str; 7] = [
    "font-style",
    "font-variant",
    "font-weight",
    "font-stretch",
    "font-size",
    "line-height",
    "font-family",
];

pub(super) fn expand_value(value: Value, important: bool) -> Vec<Declaration> {
    let rendered = super::style::render_value(&value);
    let Ok(tokens) = super::tokenize(&rendered) else {
        return Vec::new();
    };
    expand(&tokens, important).unwrap_or_default()
}

pub(super) fn expand(
    tokens: &[CssToken],
    important: bool,
) -> Result<Vec<Declaration>, CssParseError> {
    let significant: Vec<_> = tokens
        .iter()
        .enumerate()
        .filter(|(_, token)| !matches!(token, CssToken::Whitespace))
        .collect();
    if let [(.., CssToken::Ident(keyword))] = significant.as_slice()
        && matches!(
            keyword.as_str(),
            "inherit" | "initial" | "unset" | "revert" | "revert-layer"
        )
    {
        return Ok(LONGHANDS
            .iter()
            .map(|name| Declaration {
                name: (*name).into(),
                value: Value::Keyword(keyword.clone()),
                important,
            })
            .collect());
    }
    let mut values = std::collections::HashMap::new();
    for name in [
        "font-style",
        "font-variant",
        "font-weight",
        "font-stretch",
        "line-height",
    ] {
        values.insert(name, Value::Keyword("normal".into()));
    }
    let mut assigned = std::collections::HashSet::new();
    let mut index = 0;
    let size = loop {
        let (_, token) = significant
            .get(index)
            .ok_or(CssParseError::InvalidDeclaration)?;
        let percentage_is_stretch = matches!(token, CssToken::Percentage(value) if *value > 0.0)
            && significant[index + 1..]
                .iter()
                .any(|(_, candidate)| font_size(candidate).is_some());
        if !percentage_is_stretch && let Some(size) = font_size(token) {
            index += 1;
            break size;
        }
        let mut consumed = 1;
        let (name, value) = match token {
            CssToken::Ident(keyword) => match keyword.as_str() {
                "normal" => {
                    index += 1;
                    continue;
                }
                "italic" => ("font-style", Value::Keyword(keyword.clone())),
                "oblique" => {
                    let mut rendered = keyword.clone();
                    while consumed <= 2
                        && let Some((_, CssToken::Dimension(angle, unit))) =
                            significant.get(index + consumed)
                        && unit.eq_ignore_ascii_case("deg")
                        && angle.is_finite()
                        && (-90.0..=90.0).contains(angle)
                    {
                        rendered.push(' ');
                        rendered
                            .push_str(&format!("{}deg", super::tokenizer::trimmed_number(*angle)));
                        consumed += 1;
                    }
                    ("font-style", Value::Keyword(rendered))
                }
                "small-caps" => ("font-variant", Value::Keyword(keyword.clone())),
                "bold" | "bolder" | "lighter" => ("font-weight", Value::Keyword(keyword.clone())),
                "ultra-condensed" | "extra-condensed" | "condensed" | "semi-condensed"
                | "semi-expanded" | "expanded" | "extra-expanded" | "ultra-expanded" => {
                    ("font-stretch", Value::Keyword(keyword.clone()))
                }
                _ => return Err(CssParseError::InvalidDeclaration),
            },
            CssToken::Number(weight) if (1.0..=1000.0).contains(weight) => {
                ("font-weight", Value::Number(*weight))
            }
            CssToken::Percentage(stretch) if *stretch > 0.0 => {
                ("font-stretch", Value::Percentage(*stretch))
            }
            _ => return Err(CssParseError::InvalidDeclaration),
        };
        if !assigned.insert(name) {
            return Err(CssParseError::InvalidDeclaration);
        }
        values.insert(name, value);
        index += consumed;
    };
    values.insert("font-size", size);
    if matches!(significant.get(index), Some((_, CssToken::Delim('/')))) {
        index += 1;
        let (_, token) = significant
            .get(index)
            .ok_or(CssParseError::InvalidDeclaration)?;
        let height = match token {
            CssToken::Number(number) if *number >= 0.0 => Value::Number(*number),
            CssToken::Ident(keyword) if keyword == "normal" => Value::Keyword(keyword.clone()),
            _ => font_size(token)
                .filter(|v| !matches!(v, Value::Keyword(_)))
                .ok_or(CssParseError::InvalidDeclaration)?,
        };
        values.insert("line-height", height);
        index += 1;
    }
    let (family_start, _) = significant
        .get(index)
        .ok_or(CssParseError::InvalidDeclaration)?;
    for family in significant[index..].split(|(_, token)| matches!(token, CssToken::Comma)) {
        if family.is_empty()
            || !(matches!(family, [(_, CssToken::String(_))])
                || family
                    .iter()
                    .all(|(_, token)| matches!(token, CssToken::Ident(_))))
        {
            return Err(CssParseError::InvalidDeclaration);
        }
    }
    values.insert(
        "font-family",
        Value::Keyword(render_tokens(&tokens[*family_start..]).trim().into()),
    );
    Ok(LONGHANDS
        .iter()
        .map(|name| Declaration {
            name: (*name).into(),
            value: values.remove(name).unwrap(),
            important,
        })
        .collect())
}

fn font_size(token: &CssToken) -> Option<Value> {
    match token {
        CssToken::Dimension(number, unit)
            if *number >= 0.0
                && matches!(
                    unit.as_str(),
                    "px" | "em"
                        | "rem"
                        | "ex"
                        | "ch"
                        | "cap"
                        | "ic"
                        | "lh"
                        | "rlh"
                        | "vw"
                        | "vh"
                        | "vmin"
                        | "vmax"
                        | "cm"
                        | "mm"
                        | "q"
                        | "in"
                        | "pt"
                        | "pc"
                ) =>
        {
            Some(Value::Length(*number, unit.clone()))
        }
        CssToken::Percentage(number) if *number >= 0.0 => Some(Value::Percentage(*number)),
        CssToken::Number(number) if *number == 0.0 => Some(Value::Number(0.0)),
        CssToken::Ident(keyword)
            if matches!(
                keyword.as_str(),
                "xx-small"
                    | "x-small"
                    | "small"
                    | "medium"
                    | "large"
                    | "x-large"
                    | "xx-large"
                    | "xxx-large"
                    | "smaller"
                    | "larger"
            ) =>
        {
            Some(Value::Keyword(keyword.clone()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::parse_style_attribute;

    #[test]
    fn keeps_font_families_slashes_variants_and_importance() {
        let declarations = parse_style_attribute(
            "font: italic 700 24px / 1.5 AuditArabic, 'Other Face', sans-serif !important",
        );
        let value = |name| {
            declarations
                .iter()
                .find(|d| d.name == name)
                .unwrap()
                .value
                .clone()
        };
        assert_eq!(
            value("font-family"),
            Value::Keyword("AuditArabic, \"Other Face\", sans-serif".into())
        );
        assert_eq!(value("font-size"), Value::Length(24.0, "px".into()));
        assert_eq!(value("line-height"), Value::Number(1.5));
        assert_eq!(value("font-style"), Value::Keyword("italic".into()));
        assert_eq!(value("font-weight"), Value::Number(700.0));
        assert!(declarations.iter().all(|d| d.important));
        let declarations = parse_style_attribute("font:16px/24px AuditFont");
        assert!(
            declarations
                .iter()
                .any(|d| d.name == "font-family" && d.value == Value::Keyword("AuditFont".into()))
        );
        assert!(
            declarations
                .iter()
                .any(|d| d.name == "font-weight" && d.value == Value::Keyword("normal".into()))
        );
        let declarations =
            parse_style_attribute("font:oblique -10deg 700 condensed 16px AngleFace");
        assert!(declarations.iter().any(|d| {
            d.name == "font-style" && d.value == Value::Keyword("oblique -10deg".into())
        }));
        assert!(declarations.iter().any(|d| {
            d.name == "font-stretch" && d.value == Value::Keyword("condensed".into())
        }));
        let declarations = parse_style_attribute("font:75% 16px PercentageFace");
        assert!(
            declarations
                .iter()
                .any(|d| { d.name == "font-stretch" && d.value == Value::Percentage(75.0) })
        );
        assert!(
            declarations
                .iter()
                .any(|d| { d.name == "font-size" && d.value == Value::Length(16.0, "px".into()) })
        );
    }

    #[test]
    fn rejects_missing_family_and_invalid_line_height_without_partial_updates() {
        for value in [
            "16px",
            "16px/",
            "16px/-1 Face",
            "16px/huge Face",
            "bold bold 16px Face",
            "16px Face,",
            "16px 'Face' Other",
        ] {
            assert!(
                parse_style_attribute(&format!("font:{value}")).is_empty(),
                "{value}"
            );
        }
    }
}
