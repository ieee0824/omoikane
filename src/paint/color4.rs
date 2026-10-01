//! CSS Color 4 values retain their color space until rasterization.
//!
//! Conversion and gamut mapping follow https://drafts.csswg.org/css-color-4/.

use super::color::Color;

#[path = "color4/convert.rs"]
mod convert;
#[path = "color4/math.rs"]
mod math;
pub(crate) use math::Context as ColorContext;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Space {
    Hwb,
    Lab,
    Lch,
    Oklab,
    Oklch,
    Srgb,
    SrgbLinear,
    DisplayP3,
    DisplayP3Linear,
    A98Rgb,
    ProphotoRgb,
    Rec2020,
    XyzD50,
    XyzD65,
}

/// An owned, unquantized color, including missing components for CSSOM.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CssColor {
    pub space: Space,
    pub channels: [Option<f64>; 3],
    pub alpha: Option<f64>,
    expressions: [Option<String>; 4],
}

impl CssColor {
    pub fn parse(text: &str) -> Option<Self> {
        Self::parse_with_context(text, None)
    }

    pub fn parse_with_context(text: &str, context: Option<ColorContext>) -> Option<Self> {
        let lower = text.trim().to_ascii_lowercase();
        let (name, body) = lower.split_once('(')?;
        let body = body.strip_suffix(')')?;
        let pieces = math::tokens(body)?;
        let mut tokens = pieces.iter().copied();
        let space = match name {
            "hwb" => Space::Hwb,
            "lab" => Space::Lab,
            "lch" => Space::Lch,
            "oklab" => Space::Oklab,
            "oklch" => Space::Oklch,
            "color" => parse_space(tokens.next()?)?,
            _ => return None,
        };
        let mut channels = [None; 3];
        let mut expressions: [Option<String>; 4] = std::array::from_fn(|_| None);
        for (index, channel) in channels.iter_mut().enumerate() {
            let text = tokens.next()?;
            *channel = component(text, space, index, context)?;
            expressions[index] = canonical_math(text);
        }
        let alpha = match tokens.next() {
            Some("/") => {
                let text = tokens.next()?;
                expressions[3] = canonical_math(text);
                number(text, 1.0, context)?.map(|value| value.clamp(0.0, 1.0))
            }
            None => Some(1.0),
            _ => return None,
        };
        if tokens.next().is_some() {
            return None;
        }
        Some(Self {
            space,
            channels,
            alpha,
            expressions,
        })
    }

    /// Quantizes only after conversion and CSS gamut mapping to the sRGB target.
    pub fn to_color(&self) -> Color {
        let rgb = convert::to_srgb(self.space, self.channels.map(|v| v.unwrap_or(0.0)));
        let bytes = rgb.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
        Color::rgba(
            bytes[0],
            bytes[1],
            bytes[2],
            (self.alpha.unwrap_or(0.0) * 255.0).round() as u8,
        )
    }

    pub fn serialize(&self) -> String {
        self.serialize_mode(false)
    }

    fn serialize_mode(&self, computed: bool) -> String {
        if self.space == Space::Hwb
            && self.channels.iter().all(Option::is_some)
            && self.alpha.is_some()
            && !self
                .expressions
                .iter()
                .flatten()
                .any(|text| math::has_relative_units(text))
        {
            let color = self.to_color();
            let alpha = self.alpha.unwrap_or(0.0);
            return if alpha == 1.0 {
                format!("rgb({}, {}, {})", color.r, color.g, color.b)
            } else {
                format!(
                    "rgba({}, {}, {}, {})",
                    color.r,
                    color.g,
                    color.b,
                    css_number(alpha)
                )
            };
        }
        let name = match self.space {
            Space::Hwb => "hwb",
            Space::Lab => "lab",
            Space::Lch => "lch",
            Space::Oklab => "oklab",
            Space::Oklch => "oklch",
            _ => "color",
        };
        let mut components = Vec::new();
        if name == "color" {
            components.push(space_name(self.space).to_string());
        }
        for (index, component) in self.channels.into_iter().enumerate() {
            if let Some(expression) = &self.expressions[index] {
                components.push(expression.clone());
                continue;
            }
            let value = component.map_or_else(
                || "none".into(),
                |value| {
                    if self.space == Space::Hwb && index > 0 {
                        format!(
                            "{}{}",
                            css_number(value * 100.0),
                            if computed { "%" } else { "" }
                        )
                    } else {
                        css_number(value)
                    }
                },
            );
            components.push(value);
        }
        let alpha = if let Some(expression) = &self.expressions[3] {
            format!(" / {expression}")
        } else if self.alpha == Some(1.0) {
            String::new()
        } else {
            format!(
                " / {}",
                self.alpha.map_or_else(|| "none".into(), css_number)
            )
        };
        format!("{name}({}{alpha})", components.join(" "))
    }

    /// Computed serialization resolves context-free math before formatting.
    pub fn serialize_computed(&self) -> String {
        let mut resolved = self.clone();
        resolved.expressions = std::array::from_fn(|_| None);
        resolved.serialize_mode(true)
    }
}

fn canonical_math(text: &str) -> Option<String> {
    text.contains('(').then(|| math::canonical(text)).flatten()
}

fn css_number(value: f64) -> String {
    if value == 0.0 {
        return "0".into();
    }
    let scale = 10.0_f64.powf(5.0 - value.abs().log10().floor());
    let rounded = if !scale.is_finite() || scale == 0.0 {
        // Subnormal inputs must not turn into NaN during decimal rounding.
        return format!("{value:.5e}");
    } else if value == 0.0 {
        0.0
    } else {
        (value * scale).round() / scale
    };
    if rounded == 0.0 {
        "0".into()
    } else {
        rounded.to_string()
    }
}

fn parse_space(name: &str) -> Option<Space> {
    Some(match name {
        "srgb" => Space::Srgb,
        "srgb-linear" => Space::SrgbLinear,
        "display-p3" => Space::DisplayP3,
        "display-p3-linear" => Space::DisplayP3Linear,
        "a98-rgb" => Space::A98Rgb,
        "prophoto-rgb" => Space::ProphotoRgb,
        "rec2020" => Space::Rec2020,
        "xyz-d50" => Space::XyzD50,
        "xyz" | "xyz-d65" => Space::XyzD65,
        _ => return None,
    })
}

fn space_name(space: Space) -> &'static str {
    match space {
        Space::Srgb => "srgb",
        Space::SrgbLinear => "srgb-linear",
        Space::DisplayP3 => "display-p3",
        Space::DisplayP3Linear => "display-p3-linear",
        Space::A98Rgb => "a98-rgb",
        Space::ProphotoRgb => "prophoto-rgb",
        Space::Rec2020 => "rec2020",
        Space::XyzD50 => "xyz-d50",
        Space::XyzD65 => "xyz-d65",
        _ => unreachable!("only predefined spaces use color()"),
    }
}

fn component(
    text: &str,
    space: Space,
    index: usize,
    context: Option<ColorContext>,
) -> Option<Option<f64>> {
    let hue = (space == Space::Hwb && index == 0)
        || (matches!(space, Space::Lch | Space::Oklch) && index == 2);
    if hue {
        return angle(text, context);
    }
    let scale = match (space, index) {
        (Space::Hwb, _) => 100.0,
        (Space::Lab | Space::Lch, 0) => 100.0,
        (Space::Lab, _) => 125.0,
        (Space::Lch, 1) => 150.0,
        (Space::Oklab | Space::Oklch, 0) => 1.0,
        (Space::Oklab | Space::Oklch, _) => 0.4,
        _ => 1.0,
    };
    let value = number(text, scale, context)?;
    Some(value.map(|v| match (space, index) {
        (Space::Lab | Space::Lch | Space::Oklab | Space::Oklch, 0) => v.clamp(0.0, scale),
        (Space::Lch | Space::Oklch, 1) => v.max(0.0),
        (Space::Hwb, _) => v.clamp(0.0, 100.0) / 100.0,
        _ => v,
    }))
}

fn number(text: &str, percentage_scale: f64, context: Option<ColorContext>) -> Option<Option<f64>> {
    if text == "none" {
        return Some(None);
    }
    if text.contains('(') {
        let value = math::evaluate(text, percentage_scale, false, context)?;
        return Some(Some(if value.is_nan() {
            0.0
        } else {
            value.clamp(-(f32::MAX as f64), f32::MAX as f64)
        }));
    }
    let (text, scale) = text
        .strip_suffix('%')
        .map_or((text, 1.0), |v| (v, percentage_scale / 100.0));
    // Rust also accepts inf/NaN, which are not CSS numeric tokens.
    if !text
        .bytes()
        .all(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.' | b'e' | b'E'))
    {
        return None;
    }
    let mantissa = text.split(['e', 'E']).next()?;
    if mantissa.ends_with('.') {
        return None;
    }
    let value = text.parse::<f64>().ok()? * scale;
    value
        .is_finite()
        .then_some(Some(value.clamp(-(f32::MAX as f64), f32::MAX as f64)))
}

fn angle(text: &str, context: Option<ColorContext>) -> Option<Option<f64>> {
    if text.contains('(') {
        let value = math::evaluate(text, 1.0, true, context)?;
        return Some(Some(if value.is_finite() {
            value.rem_euclid(360.0)
        } else {
            0.0
        }));
    }
    if text == "none" {
        return Some(None);
    }
    for (unit, factor) in [
        ("deg", 1.0),
        ("grad", 0.9),
        ("rad", 180.0 / std::f64::consts::PI),
        ("turn", 360.0),
    ] {
        if let Some(value) = text.strip_suffix(unit) {
            return Some(number(value, 1.0, context)?.map(|v| (v * factor).rem_euclid(360.0)));
        }
    }
    if text.ends_with('%') {
        return None;
    }
    Some(number(text, 1.0, context)?.map(|v| v.rem_euclid(360.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extreme_components_are_finite_and_invalid_number_tokens_are_rejected() {
        for input in [
            "lab(50 1. 0)",
            "color(srgb 1.e2 0 0)",
            "oklab(calc(1.) 0 0)",
        ] {
            assert!(CssColor::parse(input).is_none(), "{input}");
        }
        for input in [
            "lab(50 1e300 -1e300)",
            "oklch(.5 1e300 200)",
            "color(srgb 1e300 -1e300 .5)",
            "color(srgb 1e-320 0 0)",
        ] {
            let color = CssColor::parse(input).unwrap();
            let rgb = convert::to_srgb(color.space, color.channels.map(|v| v.unwrap_or(0.0)));
            assert!(
                rgb.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)),
                "{input}: {rgb:?}"
            );
            assert!(!color.serialize().contains("NaN"), "{input}");
        }
    }

    #[test]
    fn parses_missing_percentages_angles_and_alpha() {
        for (input, expected) in [
            ("hwb(.5turn 20% 30% / 50%)", "rgba(51, 179, 179, 0.5)"),
            ("lab(50% 100% -100% / none)", "lab(50 125 -125 / none)"),
            ("lch(50% 100% 400grad)", "lch(50 150 0)"),
            ("oklab(50% 100% none)", "oklab(0.5 0.4 none)"),
            ("oklch(50% 100% 180deg)", "oklch(0.5 0.4 180)"),
            (
                "color(xyz none 50% -1 / 200%)",
                "color(xyz-d65 none 0.5 -1)",
            ),
        ] {
            assert_eq!(
                CssColor::parse(input).unwrap().serialize(),
                expected,
                "{input}"
            );
        }
    }

    #[test]
    fn rejects_invalid_grammars() {
        for text in [
            "hwb(0 0px 0)",
            "lab(0, 0, 0)",
            "lab(0 0)",
            "oklab(0 0 0 0)",
            "lch(50 10 10%)",
            "color(unknown 1 0 0)",
            "color(srgb 1 0 0 /)",
            "lab(NaN 0 0)",
            "lab(0 0 inf)",
            "lab(0 0 0 / 1 / 1)",
            "color(srgb 1 0 0)junk",
        ] {
            assert!(CssColor::parse(text).is_none(), "{text}");
        }
    }

    #[test]
    fn neutral_colors_and_hwb_normalization() {
        for text in [
            "lab(100 0 0)",
            "lch(100 0 none)",
            "oklab(1 0 0)",
            "oklch(1 0 none)",
            "color(display-p3 1 1 1)",
            "color(prophoto-rgb 1 1 1)",
            "color(rec2020 1 1 1)",
            "color(a98-rgb 1 1 1)",
        ] {
            assert_eq!(
                CssColor::parse(text).unwrap().to_color(),
                Color::rgb(255, 255, 255),
                "{text}"
            );
        }
        assert_eq!(
            CssColor::parse("hwb(0 80% 80%)").unwrap().to_color(),
            Color::rgb(128, 128, 128)
        );
        assert_eq!(
            CssColor::parse("color(srgb 1 0 0 / none)")
                .unwrap()
                .to_color(),
            Color::rgba(255, 0, 0, 0)
        );
    }
}

#[cfg(test)]
mod math_tests {
    use super::*;

    #[test]
    fn math_comparisons_propagate_nan_before_color_resolution() {
        for (input, specified, computed) in [
            (
                "oklab(min(NaN, .5) 0 0)",
                "oklab(calc(NaN) 0 0)",
                "oklab(0 0 0)",
            ),
            ("lab(max(NaN, 50) 0 0)", "lab(calc(NaN) 0 0)", "lab(0 0 0)"),
            (
                "lab(clamp(0, NaN, 100) 0 0)",
                "lab(calc(NaN) 0 0)",
                "lab(0 0 0)",
            ),
        ] {
            let color = CssColor::parse(input).unwrap();
            assert_eq!(color.serialize(), specified, "{input}");
            assert_eq!(color.serialize_computed(), computed, "{input}");
        }
    }

    #[test]
    fn numeric_math_resolves_for_computed_colors_but_is_retained_in_cssom() {
        let color =
            CssColor::parse("lab(calc(50 * 3) calc(0.5 - 1) calc(1.5) / calc(-0.5 + 1))").unwrap();
        assert_eq!(
            color.serialize(),
            "lab(calc(150) calc(-0.5) calc(1.5) / calc(0.5))"
        );
        assert_eq!(color.serialize_computed(), "lab(100 -0.5 1.5 / 0.5)");
        let color =
            CssColor::parse("color(srgb calc(50% * 3) calc(-150% / 3) calc(50%) / calc(-50% * 3))")
                .unwrap();
        assert_eq!(color.serialize_computed(), "color(srgb 1.5 -0.5 0.5 / 0)");
        assert_eq!(
            CssColor::parse("hwb(calc(1turn / 2) 20 30)")
                .unwrap()
                .serialize(),
            "rgb(51, 179, 179)"
        );
        assert!(CssColor::parse("lab(calc(20px) 0 0)").is_none());
        assert!(CssColor::parse("lab(calc(1+2) 0 0)").is_none());
    }
}

#[cfg(test)]
mod context_tests {
    use super::*;

    #[test]
    fn relative_math_keeps_dependencies_until_context_is_supplied() {
        let text = "color(srgb calc(50% + (sign(1em - 10px) * 10%)) 0 0 / 0.5)";
        let color = CssColor::parse(text).unwrap();
        assert_eq!(
            color.serialize(),
            "color(srgb calc(50% + (10% * sign(1em - 10px))) 0 0 / 0.5)"
        );
        for (font, width, expected) in [
            (16.0, 1000.0, "color(srgb 0.6 0 0 / 0.5)"),
            (5.0, 100.0, "color(srgb 0.4 0 0 / 0.5)"),
        ] {
            let context = ColorContext {
                font,
                root_font: 16.0,
                viewport: [800.0, 600.0],
                container: [width, 100.0],
            };
            assert_eq!(
                CssColor::parse_with_context(text, Some(context))
                    .unwrap()
                    .serialize_computed(),
                expected
            );
            let container_text = text.replace("1em", "2cqw");
            assert_eq!(
                CssColor::parse_with_context(&container_text, Some(context))
                    .unwrap()
                    .serialize_computed(),
                expected
            );
        }
    }

    #[test]
    fn hwb_rounding_and_missing_components_use_cssom_stage() {
        assert_eq!(
            CssColor::parse("hwb(120 30% 50%)").unwrap().to_color(),
            Color::rgb(77, 128, 77)
        );
        let color = CssColor::parse("hwb(120 80% none)").unwrap();
        assert_eq!(color.serialize(), "hwb(120 80 none)");
        assert_eq!(color.serialize_computed(), "hwb(120 80% none)");
        assert_eq!(
            CssColor::parse("hwb(0 0 0 / .5)").unwrap().to_color().a,
            128
        );
    }
}

#[cfg(test)]
mod conversion_reference_tests {
    use super::*;

    #[test]
    fn conversions_and_css_minde_mapping_match_independent_reference() {
        let reference: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/css-color4/gamut-reference.json"
        ))
        .unwrap();
        for record in reference["records"].as_array().unwrap() {
            let text = record["input"].as_str().unwrap();
            let parsed = CssColor::parse(text).unwrap();
            let mapped = convert::to_srgb(parsed.space, parsed.channels.map(|v| v.unwrap_or(0.0)));
            for (actual, expected) in mapped
                .into_iter()
                .zip(record["mapped_srgb"].as_array().unwrap())
            {
                // f64 matrices and the independent reference agree within
                // roundoff; compare before quantization at half-byte boundaries.
                assert!(
                    (actual - expected.as_f64().unwrap()).abs() < 1e-12,
                    "{text}: {actual} vs {expected}"
                );
            }
            let bytes = record
                .get("expected_canvas_pixel")
                .unwrap_or(&record["pixel"])
                .as_array()
                .unwrap();
            let expected = Color::rgba(
                bytes[0].as_u64().unwrap() as u8,
                bytes[1].as_u64().unwrap() as u8,
                bytes[2].as_u64().unwrap() as u8,
                bytes[3].as_u64().unwrap() as u8,
            );
            assert_eq!(
                CssColor::parse(text).unwrap().to_color(),
                expected,
                "{text}"
            );
        }
    }
}
