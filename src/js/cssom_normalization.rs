//! Pure CSSOM specified-value validation and serialization.

const VALIDATED_PROPERTIES: &[&str] = &[
    "all",
    "accent-color",
    "appearance",
    "caret-color",
    "color",
    "color-scheme",
    "field-sizing",
    "resize",
    "touch-action",
    "user-select",
    "background-color",
    "border-color",
    "border-top-color",
    "border-right-color",
    "border-bottom-color",
    "border-left-color",
    "outline-color",
    "outline-offset",
    "text-decoration",
    "text-decoration-color",
    "text-decoration-thickness",
    "paint-order",
    "clip-path",
    "-webkit-clip-path",
    "shape-outside",
    "shape-margin",
    "mask",
    "-webkit-mask",
    "mask-image",
    "-webkit-mask-image",
    "mask-mode",
    "-webkit-mask-mode",
    "mask-composite",
    "-webkit-mask-composite",
    "transform-style",
    "backface-visibility",
    "mix-blend-mode",
    "isolation",
    "text-overflow",
    "tab-size",
    "content-visibility",
    "opacity",
    "width",
    "height",
    "min-width",
    "min-height",
    "max-width",
    "max-height",
    "inline-size",
    "block-size",
    "min-inline-size",
    "min-block-size",
    "max-inline-size",
    "max-block-size",
    "top",
    "right",
    "bottom",
    "left",
    "inset",
    "inset-inline",
    "inset-block",
    "inset-inline-start",
    "inset-inline-end",
    "inset-block-start",
    "inset-block-end",
    "border-inline-start-style",
    "border-inline-end-style",
    "border-block-start-style",
    "border-block-end-style",
    "border-inline-style",
    "border-block-style",
    "border-inline-start-color",
    "border-inline-end-color",
    "border-block-start-color",
    "border-block-end-color",
    "border-inline-color",
    "border-block-color",
    "border-start-start-radius",
    "border-start-end-radius",
    "border-end-start-radius",
    "border-end-end-radius",
    "border-image",
    "border-image-source",
    "border-image-slice",
    "border-image-width",
    "border-image-outset",
    "border-image-repeat",
    "mask-border",
    "mask-border-source",
    "mask-border-slice",
    "mask-border-width",
    "mask-border-outset",
    "mask-border-repeat",
    "mask-border-mode",
    "object-position",
    "contain-intrinsic-size",
    "contain-intrinsic-width",
    "contain-intrinsic-height",
    "contain-intrinsic-inline-size",
    "contain-intrinsic-block-size",
    "columns",
    "column-count",
    "column-width",
    "column-fill",
    "column-span",
    "column-gap",
    "column-rule",
    "column-rule-color",
    "column-rule-style",
    "column-rule-width",
    "break-before",
    "break-after",
    "break-inside",
    "box-decoration-break",
    "orphans",
    "widows",
    "counter-reset",
    "counter-increment",
    "counter-set",
    "quotes",
    "scroll-behavior",
    "overscroll-behavior",
    "overscroll-behavior-x",
    "overscroll-behavior-y",
    "overscroll-behavior-inline",
    "overscroll-behavior-block",
];

pub(super) fn normalize(property: &str, value: &str) -> Option<String> {
    if property == "animation-composition" {
        return crate::css::style::normalize_animation_composition_specified(value);
    }
    if matches!(
        property,
        "position-area" | "position-try-fallbacks" | "position-visibility"
    ) || (!property.starts_with("--") && value.to_ascii_lowercase().contains("anchor-size("))
    {
        return crate::css::style::normalize_anchor_position_specified(property, value);
    }
    if property == "paint-order" {
        return crate::css::style::normalize_paint_order_specified(value);
    }
    if matches!(property, "clip-path" | "-webkit-clip-path") {
        return crate::css::style::normalize_clip_path_specified(value);
    }
    if matches!(
        property,
        "accent-color"
            | "appearance"
            | "caret-color"
            | "color-scheme"
            | "field-sizing"
            | "resize"
            | "touch-action"
            | "user-select"
    ) {
        return crate::css::style::normalize_css_ui_specified(property, value);
    }
    if matches!(
        property,
        "scrollbar-width"
            | "scrollbar-gutter"
            | "scrollbar-color"
            | "overflow-anchor"
            | "overflow-clip-margin"
    ) {
        return crate::css::style::normalize_scroll_ui_specified(property, value);
    }
    if matches!(
        property,
        "border-image-slice"
            | "border-image-width"
            | "border-image-outset"
            | "border-image-repeat"
            | "mask-border-slice"
            | "mask-border-width"
            | "mask-border-outset"
            | "mask-border-repeat"
    ) {
        return crate::css::style::normalize_nine_slice_specified(property, value);
    }
    if matches!(
        property,
        "text-emphasis-position" | "text-autospace" | "text-spacing-trim"
    ) {
        return crate::css::style::normalize_cjk_specified(property, value);
    }
    if matches!(property, "initial-letter" | "text-box-edge") {
        if !crate::css::style::supports_declaration(property, value) {
            return None;
        }
        let keyword = value.trim().to_ascii_lowercase();
        if matches!(
            keyword.as_str(),
            "initial" | "inherit" | "unset" | "revert" | "revert-layer"
        ) {
            return Some(keyword);
        }
        let declarations = crate::css::parse_style_attribute(&format!("{property}: {value}"));
        let declaration = declarations
            .iter()
            .find(|declaration| declaration.name == property)?;
        return if property == "initial-letter" {
            crate::css::style::normalize_initial_letter_specified(&declaration.value)
        } else {
            crate::css::style::normalize_text_box_edge_specified(&declaration.value)
        };
    }
    if property == "hyphenate-character" {
        let declarations = crate::css::parse_style_attribute(&format!("{property}: {value}"));
        let declaration = declarations
            .iter()
            .find(|declaration| declaration.name == property)?;
        return match &declaration.value {
            crate::css::Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("auto") => {
                Some("auto".to_string())
            }
            crate::css::Value::String(string) => Some(serialize_css_string(string)),
            _ => None,
        };
    }
    if matches!(
        property,
        "font-feature-settings" | "font-variation-settings"
    ) {
        let declarations = crate::css::parse_style_attribute(&format!("{property}: {value}"));
        let declaration = declarations
            .iter()
            .find(|declaration| declaration.name == property)?;
        return crate::css::style::normalize_opentype_settings_specified(
            property,
            &declaration.value,
        );
    }
    if matches!(
        property,
        "font-synthesis" | "font-variant-east-asian" | "font-variant-alternates"
    ) {
        let declarations = crate::css::parse_style_attribute(&format!("{property}: {value}"));
        let declaration = declarations
            .iter()
            .find(|declaration| declaration.name == property)?;
        return crate::css::style::normalize_font_keyword_list_specified(
            property,
            &declaration.value,
        );
    }
    if property == "font-size-adjust" {
        let declarations = crate::css::parse_style_attribute(&format!("{property}: {value}"));
        let declaration = declarations
            .iter()
            .find(|declaration| declaration.name == property)?;
        return crate::css::style::normalize_font_size_adjust_specified(&declaration.value);
    }
    if matches!(
        property,
        "counter-reset" | "counter-increment" | "counter-set"
    ) {
        return crate::css::style::normalize_counter_specified(property, value);
    }
    if (value
        .as_bytes()
        .windows(3)
        .any(|word| word.eq_ignore_ascii_case(b"var"))
        || value.contains('\\'))
        && crate::css::parse_style_attribute(&format!("{property}: {value}"))
            .iter()
            .any(|declaration| crate::css::style::value_contains_var_function(&declaration.value))
    {
        return crate::css::style::supports_declaration(property, value)
            .then(|| value.trim().to_owned());
    }
    if let Some(value) = crate::css::style::normalize_tree_counting_numeric(property, value) {
        return Some(value);
    }
    if let Some(value) = crate::css::style::normalize_numeric_math_specified(property, value) {
        return Some(value);
    }
    if matches!(property, "translate" | "rotate" | "scale") {
        return crate::css::style::individual_transform::normalize_specified(property, value);
    }
    if matches!(
        property,
        "order" | "grid-auto-flow" | "grid-auto-rows" | "grid-auto-columns"
    ) {
        return crate::css::style::grid_properties::normalize_specified(property, value);
    }
    if property == "text-shadow" {
        crate::css::style::text_shadow::normalize_specified(value)
    } else if matches!(property, "text-indent" | "word-spacing") {
        crate::css::style::normalize_text_spacing_value(property, value)
    } else if matches!(
        property,
        "text-underline-position" | "text-underline-offset"
    ) {
        crate::css::style::normalize_underline_value(property, value)
    } else if matches!(
        property,
        "inset"
            | "inset-inline"
            | "inset-block"
            | "inset-inline-start"
            | "inset-inline-end"
            | "inset-block-start"
            | "inset-block-end"
            | "margin-inline"
            | "margin-block"
            | "margin-inline-start"
            | "margin-inline-end"
            | "margin-block-start"
            | "margin-block-end"
            | "padding-inline"
            | "padding-block"
            | "padding-inline-start"
            | "padding-inline-end"
            | "padding-block-start"
            | "padding-block-end"
    ) {
        crate::css::style::normalize_box_lengths(property, value)
    } else if matches!(
        property,
        "border-inline-start-width"
            | "border-inline-end-width"
            | "border-block-start-width"
            | "border-block-end-width"
            | "border-inline-width"
            | "border-block-width"
    ) {
        crate::css::style::normalize_logical_border_width(property, value)
    } else if matches!(
        property,
        "border-inline-start-color"
            | "border-inline-end-color"
            | "border-block-start-color"
            | "border-block-end-color"
            | "border-inline-color"
            | "border-block-color"
    ) {
        crate::css::style::normalize_logical_border_color(property, value)
    } else if matches!(
        property,
        "border-inline"
            | "border-block"
            | "border-inline-start"
            | "border-inline-end"
            | "border-block-start"
            | "border-block-end"
    ) {
        crate::css::style::normalize_logical_border_shorthand(property, value)
    } else if property == "transition" {
        crate::css::normalize_transition_shorthand(value)
    } else if matches!(
        property,
        "transition-property"
            | "transition-duration"
            | "transition-timing-function"
            | "transition-delay"
    ) {
        if matches!(property, "transition-duration" | "transition-delay")
            && value.contains('(')
            && crate::css::style::supports_declaration(property, value)
        {
            Some(value.to_string())
        } else {
            crate::css::normalize_transition_longhand(property, value)
        }
    } else {
        normalize_validated_or_unchecked(property, value)
    }
}

pub(super) fn serialize_css_string(value: &str) -> String {
    let mut serialized = String::with_capacity(value.len() + 2);
    serialized.push('"');
    for ch in value.chars() {
        match ch {
            '"' | '\\' => {
                serialized.push('\\');
                serialized.push(ch);
            }
            '\0' => serialized.push_str("\\fffd "),
            ch if ch.is_control() => {
                serialized.push('\\');
                serialized.push_str(&format!("{:x} ", ch as u32));
            }
            ch => serialized.push(ch),
        }
    }
    serialized.push('"');
    serialized
}

fn normalize_validated_or_unchecked(property: &str, value: &str) -> Option<String> {
    if !VALIDATED_PROPERTIES.contains(&property) {
        return Some(value.to_string());
    }
    crate::css::supports_declaration(property, value).then(|| {
        if property == "overscroll-behavior" {
            let values = value.split_whitespace().collect::<Vec<_>>();
            if values.len() == 2 && values[0].eq_ignore_ascii_case(values[1]) {
                values[0].to_ascii_lowercase()
            } else {
                value.to_ascii_lowercase()
            }
        } else if matches!(
            property,
            "column-width" | "column-gap" | "column-rule-width" | "shape-margin"
        ) && value.trim() == "0"
        {
            "0px".to_string()
        } else {
            crate::paint::color4::CssColor::parse(value)
                .map_or_else(|| value.to_string(), |color| color.serialize())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn specified_number_math_is_canonicalized_at_the_cssom_boundary() {
        assert_eq!(
            normalize("opacity", "round(1.1,1)").as_deref(),
            Some("calc(1)")
        );
        assert_eq!(
            normalize("transform", "scale(calc(log(0)))").as_deref(),
            Some("scale(calc(-infinity))")
        );
    }

    #[test]
    fn property_boundaries_finish_numeric_math_normalization() {
        assert_eq!(normalize("tab-size", "calc(40% + 50px)"), None);
        assert_eq!(
            normalize("scale", "calc(2 * 1) 3 calc(4 * 100%)").as_deref(),
            Some("calc(2) 3 calc(400%)")
        );
        assert_eq!(
            normalize("color", "color(xyz 0 calc(infinity) 0)").as_deref(),
            Some("color(xyz-d65 0 calc(infinity) 0)")
        );
        assert_eq!(
            normalize("color", "hwb(calc(infinity) 20% 10%)").as_deref(),
            Some("rgb(230, 51, 51)")
        );
        assert_eq!(
            normalize(
                "color",
                "lch(calc(50 * 3) calc(0.5 - 1) calc(20deg * 2) / calc(-0.5 + 1))",
            )
            .as_deref(),
            Some("lch(calc(150) calc(-0.5) calc(40deg) / calc(0.5))")
        );
    }

    #[test]
    fn css_ui_values_validate_and_use_canonical_specified_serialization() {
        assert_eq!(
            normalize("accent-color", "#fff").as_deref(),
            Some("rgb(255, 255, 255)")
        );
        assert_eq!(
            normalize("color-scheme", "only light").as_deref(),
            Some("light only")
        );
        assert_eq!(
            normalize("touch-action", "pan-y pan-x").as_deref(),
            Some("pan-x pan-y")
        );
        assert_eq!(normalize("field-sizing", "auto"), None);
        assert_eq!(normalize("user-select", "contain"), None);
    }

    #[test]
    fn scroll_ui_values_validate_and_use_canonical_specified_serialization() {
        assert_eq!(
            normalize("scrollbar-gutter", "both-edges stable").as_deref(),
            Some("stable both-edges")
        );
        assert_eq!(
            normalize("scrollbar-color", "red blue").as_deref(),
            Some("red blue")
        );
        assert_eq!(
            normalize("scrollbar-color", "#ff0000 #00ff00").as_deref(),
            Some("rgb(255, 0, 0) rgb(0, 255, 0)")
        );
        assert_eq!(
            normalize("overflow-clip-margin", "padding-box 2em").as_deref(),
            Some("padding-box 2em")
        );
        assert_eq!(normalize("scrollbar-width", "medium"), None);
        assert_eq!(normalize("overflow-anchor", "always"), None);
    }
}
