//! Pure CSSOM specified-value validation and serialization.

const VALIDATED_PROPERTIES: &[&str] = &[
    "all",
    "color",
    "background-color",
    "border-color",
    "border-top-color",
    "border-right-color",
    "border-bottom-color",
    "border-left-color",
    "outline-color",
    "text-decoration",
    "text-decoration-color",
    "text-decoration-thickness",
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
    "content-visibility",
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
    "scroll-behavior",
    "overscroll-behavior",
    "overscroll-behavior-x",
    "overscroll-behavior-y",
    "overscroll-behavior-inline",
    "overscroll-behavior-block",
];

pub(super) fn normalize(property: &str, value: &str) -> Option<String> {
    if matches!(
        property,
        "order" | "grid-auto-flow" | "grid-auto-rows" | "grid-auto-columns"
    ) {
        return crate::css::style::grid_properties::normalize_specified(property, value);
    }
    if property == "text-shadow" {
        crate::css::style::text_shadow::normalize_specified(value)
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
    ) {
        crate::css::style::normalize_logical_inset(property, value)
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
        crate::css::normalize_transition_longhand(property, value)
    } else {
        normalize_validated_or_unchecked(property, value)
    }
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
