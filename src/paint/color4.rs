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
    specified: Option<String>,
}

impl CssColor {
    pub fn parse(text: &str) -> Option<Self> {
        Self::parse_with_context(text, None)
    }

    pub fn parse_with_context(text: &str, context: Option<ColorContext>) -> Option<Self> {
        let lower = text.trim().to_ascii_lowercase();
        if lower.starts_with("color-mix(") {
            return parse_color_mix(&lower, context);
        }
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
            specified: None,
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
        if let Some(specified) = &self.specified {
            return specified.clone();
        }
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
                            if computed
                                || !self
                                    .expressions
                                    .iter()
                                    .flatten()
                                    .any(|text| math::has_relative_units(text))
                            {
                                "%"
                            } else {
                                ""
                            }
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

#[derive(Debug)]
struct MixComponent {
    color: CssColor,
    percentage: Option<f64>,
    percentage_was_specified: bool,
    specified_color: String,
}

fn parse_color_mix(text: &str, context: Option<ColorContext>) -> Option<CssColor> {
    let body = text.strip_prefix("color-mix(")?.strip_suffix(')')?;
    let parts = crate::css::split_top_level_commas(body);
    if parts.len() < 2 {
        return None;
    }
    let (space, is_hsl, hue_method, first_color) = parse_interpolation_method(parts[0])?;
    let color_parts = if first_color.is_empty() {
        &parts[1..]
    } else {
        &parts[..]
    };
    if color_parts.len() < 2 {
        return None;
    }
    let mut components = color_parts
        .iter()
        .enumerate()
        .map(|(index, part)| {
            let text = if index == 0 && !first_color.is_empty() {
                first_color
            } else {
                part.trim()
            };
            parse_mix_component(text, context)
        })
        .collect::<Option<Vec<_>>>()?;
    normalize_mix_percentages(&mut components)?;
    let specified = serialize_specified_mix(parts[0], &components);
    let mut color = mix_css_colors(space, is_hsl, hue_method, &components)?;
    color.specified = Some(specified);
    Some(color)
}

fn parse_interpolation_method(first: &str) -> Option<(Space, bool, &str, &str)> {
    let first = first.trim();
    if !first.starts_with("in ") {
        return Some((Space::Oklab, false, "shorter", first));
    }
    let words = first.split_ascii_whitespace().collect::<Vec<_>>();
    let space_name = *words.get(1)?;
    let is_hsl = space_name == "hsl";
    let space = match space_name {
        "hsl" | "hwb" => Space::Hwb,
        "lab" => Space::Lab,
        "lch" => Space::Lch,
        "oklab" => Space::Oklab,
        "oklch" => Space::Oklch,
        name => parse_space(name)?,
    };
    let hue_method = match words.get(2..) {
        Some([]) => "shorter",
        Some([method, "hue"])
            if matches!(*method, "shorter" | "longer" | "increasing" | "decreasing") =>
        {
            method
        }
        _ => return None,
    };
    Some((space, is_hsl, hue_method, ""))
}

fn parse_mix_component(text: &str, context: Option<ColorContext>) -> Option<MixComponent> {
    let words = crate::css::split_top_level_whitespace_strict(text)?;
    if words.is_empty() {
        return None;
    }
    let percentage_at_start = parse_mix_percentage(words[0], context);
    let percentage_at_end = words
        .last()
        .filter(|_| words.len() > 1)
        .and_then(|word| parse_mix_percentage(word, context));
    if percentage_at_start.is_some() && percentage_at_end.is_some() {
        return None;
    }
    let (color_words, percentage) = if let Some(value) = percentage_at_start {
        (&words[1..], Some(value))
    } else if let Some(value) = percentage_at_end {
        (&words[..words.len() - 1], Some(value))
    } else {
        (&words[..], None)
    };
    if color_words.len() != 1 {
        return None;
    }
    let color = parse_absolute_mix_color(color_words[0], context)?;
    let specified_color = specified_mix_color(color_words[0], &color);
    Some(MixComponent {
        color,
        percentage,
        percentage_was_specified: percentage.is_some(),
        specified_color,
    })
}

fn specified_mix_color(input: &str, color: &CssColor) -> String {
    let lower = input.to_ascii_lowercase();
    if lower.starts_with("hsl(") || lower.starts_with("hsla(") {
        let rgba = color.to_color();
        return if rgba.a == 255 {
            format!("rgb({}, {}, {})", rgba.r, rgba.g, rgba.b)
        } else {
            format!(
                "rgba({}, {}, {}, {})",
                rgba.r,
                rgba.g,
                rgba.b,
                css_number(f64::from(rgba.a) / 255.0)
            )
        };
    }
    if lower.starts_with("color-mix(")
        || matches!(
            lower.split_once('(').map(|(name, _)| name),
            Some("hwb" | "lab" | "lch" | "oklab" | "oklch" | "color")
        )
    {
        color.serialize()
    } else {
        lower
    }
}

fn serialize_specified_mix(method_or_color: &str, components: &[MixComponent]) -> String {
    let method = method_or_color.trim();
    let method = if method.starts_with("in ") {
        method.strip_suffix(" shorter hue").unwrap_or(method)
    } else {
        ""
    };
    let omit_percentages = components.len() == 2
        && components
            .iter()
            .all(|component| component.percentage == Some(50.0));
    let any_percentage = components
        .iter()
        .any(|component| component.percentage_was_specified);
    let colors = components
        .iter()
        .map(|component| {
            if omit_percentages || !any_percentage {
                component.specified_color.clone()
            } else {
                format!(
                    "{} {}%",
                    component.specified_color,
                    css_number(component.percentage.unwrap())
                )
            }
        })
        .collect::<Vec<_>>();
    if method.is_empty() {
        format!("color-mix({})", colors.join(", "))
    } else {
        format!("color-mix({method}, {})", colors.join(", "))
    }
}

fn parse_mix_percentage(text: &str, context: Option<ColorContext>) -> Option<f64> {
    if !text.ends_with('%') && !text.starts_with("calc(") {
        return None;
    }
    let value = number(text, 100.0, context)??;
    (value.is_finite() && (0.0..=100.0).contains(&value)).then_some(value)
}

fn parse_absolute_mix_color(text: &str, context: Option<ColorContext>) -> Option<CssColor> {
    if text.starts_with("color-mix(")
        || matches!(
            text.split_once('(').map(|(name, _)| name),
            Some("hwb" | "lab" | "lch" | "oklab" | "oklch" | "color")
        )
    {
        return CssColor::parse_with_context(text, context);
    }
    let color = super::color::parse_color(text)?;
    Some(CssColor {
        space: Space::Srgb,
        channels: [color.r, color.g, color.b].map(|value| Some(f64::from(value) / 255.0)),
        alpha: Some(f64::from(color.a) / 255.0),
        expressions: std::array::from_fn(|_| None),
        specified: None,
    })
}

fn normalize_mix_percentages(components: &mut [MixComponent]) -> Option<()> {
    let specified = components
        .iter()
        .filter_map(|item| item.percentage)
        .sum::<f64>();
    let missing = components
        .iter()
        .filter(|item| item.percentage.is_none())
        .count();
    if specified > 100.0 && missing > 0 {
        return None;
    }
    let default = if missing == 0 {
        0.0
    } else {
        (100.0 - specified) / missing as f64
    };
    for component in components {
        component.percentage.get_or_insert(default);
    }
    Some(())
}

fn mix_css_colors(
    space: Space,
    is_hsl: bool,
    hue_method: &str,
    components: &[MixComponent],
) -> Option<CssColor> {
    let sum = components
        .iter()
        .map(|item| item.percentage.unwrap())
        .sum::<f64>();
    let alpha_multiplier = (sum / 100.0).min(1.0);
    let hue_index = match space {
        Space::Hwb => Some(0),
        Space::Lch | Space::Oklch => Some(2),
        _ => None,
    };
    let converted = components
        .iter()
        .map(|item| {
            let rgb = convert::to_srgb(
                item.color.space,
                item.color.channels.map(|value| value.unwrap_or(0.0)),
            );
            let channels = if is_hsl {
                rgb_to_hsl(rgb)
            } else {
                convert::from_srgb(space, rgb)
            };
            (
                channels,
                item.color.alpha.unwrap_or(0.0),
                item.percentage.unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let reference_hue = hue_index.and_then(|index| {
        converted
            .iter()
            .find(|(channels, _, _)| !powerless_hue(space, is_hsl, channels))
            .map(|(channels, _, _)| channels[index])
    });
    let mut premultiplied = [0.0; 3];
    let mut alpha = 0.0;
    let mut hue_total = 0.0;
    for (component_index, (mut channels, source_alpha, percentage)) in
        converted.into_iter().enumerate()
    {
        let normalized = if sum == 0.0 {
            1.0 / components.len() as f64
        } else {
            percentage / sum
        };
        if let (Some(index), Some(reference)) = (hue_index, reference_hue) {
            channels[index] = if powerless_hue(space, is_hsl, &channels) {
                reference
            } else if component_index == 0 {
                reference
            } else {
                adjusted_hue(reference, channels[index], hue_method)?
            };
            hue_total += channels[index] * normalized;
        }
        alpha += source_alpha * normalized;
        for index in 0..3 {
            if Some(index) != hue_index {
                premultiplied[index] += channels[index] * source_alpha * normalized;
            }
        }
    }
    let mut channels = [0.0; 3];
    if alpha > 0.0 {
        for index in 0..3 {
            channels[index] = if Some(index) == hue_index {
                hue_total
            } else {
                premultiplied[index] / alpha
            };
        }
    }
    if let Some(index) = hue_index {
        channels[index] = channels[index].rem_euclid(360.0);
    }
    let (space, channels) = if is_hsl {
        (Space::Srgb, hsl_to_rgb(channels))
    } else if space == Space::Hwb {
        (Space::Srgb, convert::to_srgb(space, channels))
    } else {
        (space, channels)
    };
    Some(CssColor {
        space,
        channels: channels.map(Some),
        alpha: Some(alpha * alpha_multiplier),
        expressions: std::array::from_fn(|_| None),
        specified: None,
    })
}

/// Mixes concrete CPAL colors in the CSS `palette-mix()` interpolation space.
pub(crate) fn mix_palette_colors(method: &str, colors: &[([u8; 4], f32)]) -> Option<[u8; 4]> {
    let mut words = method.split_ascii_whitespace();
    if !words.next()?.eq_ignore_ascii_case("in") {
        return None;
    }
    let space_name = words.next()?.to_ascii_lowercase();
    let is_hsl = space_name == "hsl";
    let space = match space_name.as_str() {
        "hsl" => Space::Hwb,
        "hwb" => Space::Hwb,
        "lab" => Space::Lab,
        "lch" => Space::Lch,
        "oklab" => Space::Oklab,
        "oklch" => Space::Oklch,
        "srgb" => Space::Srgb,
        "srgb-linear" => Space::SrgbLinear,
        "display-p3" => Space::DisplayP3,
        "a98-rgb" => Space::A98Rgb,
        "prophoto-rgb" => Space::ProphotoRgb,
        "rec2020" => Space::Rec2020,
        "xyz" | "xyz-d65" => Space::XyzD65,
        "xyz-d50" => Space::XyzD50,
        _ => return None,
    };
    let hue_method = match (words.next(), words.next(), words.next()) {
        (None, None, None) => "shorter",
        (Some(method), Some("hue"), None) => method,
        _ => return None,
    };
    if colors.is_empty()
        || colors
            .iter()
            .any(|(_, weight)| !weight.is_finite() || *weight < 0.0)
    {
        return None;
    }
    let sum = colors.iter().map(|(_, weight)| *weight as f64).sum::<f64>();
    if sum <= 0.0 {
        return None;
    }
    let alpha_multiplier = (sum / 100.0).min(1.0);
    let hue_index = match space {
        Space::Hwb => Some(0),
        Space::Lch | Space::Oklch => Some(2),
        _ => None,
    };
    let converted = colors
        .iter()
        .map(|(rgba, weight)| {
            let rgb = [rgba[0], rgba[1], rgba[2]].map(|value| f64::from(value) / 255.0);
            let channels = if is_hsl {
                rgb_to_hsl(rgb)
            } else {
                convert::from_srgb(space, rgb)
            };
            (channels, f64::from(rgba[3]) / 255.0, f64::from(*weight))
        })
        .collect::<Vec<_>>();
    // An achromatic component has a missing hue. CSS hue fixup borrows a
    // present hue before interpolation instead of treating it as zero degrees.
    let reference_hue = hue_index.and_then(|index| {
        converted
            .iter()
            .find(|(channels, _, _)| !powerless_hue(space, is_hsl, channels))
            .map(|(channels, _, _)| channels[index])
    });
    let mut premultiplied = [0.0; 3];
    let mut alpha = 0.0;
    let mut hue_total = 0.0;
    for (component_index, (mut channels, source_alpha, weight)) in converted.into_iter().enumerate()
    {
        let normalized = weight / sum;
        if let (Some(index), Some(reference)) = (hue_index, reference_hue) {
            channels[index] = if powerless_hue(space, is_hsl, &channels) {
                reference
            } else if component_index == 0 {
                reference
            } else {
                adjusted_hue(reference, channels[index], hue_method)?
            };
            hue_total += channels[index] * normalized;
        }
        alpha += source_alpha * normalized;
        for index in 0..3 {
            if Some(index) != hue_index {
                premultiplied[index] += channels[index] * source_alpha * normalized;
            }
        }
    }
    let mut channels = [0.0; 3];
    if alpha > 0.0 {
        for index in 0..3 {
            channels[index] = if Some(index) == hue_index {
                hue_total
            } else {
                premultiplied[index] / alpha
            };
        }
    }
    if let Some(index) = hue_index {
        channels[index] = channels[index].rem_euclid(360.0);
    }
    let rgb = if is_hsl {
        hsl_to_rgb(channels)
    } else {
        convert::to_srgb(space, channels)
    };
    Some([
        (rgb[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgb[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgb[2].clamp(0.0, 1.0) * 255.0).round() as u8,
        (alpha * alpha_multiplier * 255.0).round().clamp(0.0, 255.0) as u8,
    ])
}

fn powerless_hue(space: Space, is_hsl: bool, channels: &[f64; 3]) -> bool {
    if is_hsl {
        channels[1].abs() <= 1e-12
    } else {
        match space {
            Space::Hwb => channels[1] + channels[2] >= 1.0 - 1e-12,
            Space::Lch | Space::Oklch => channels[1].abs() <= 1e-12,
            _ => false,
        }
    }
}

fn rgb_to_hsl(rgb: [f64; 3]) -> [f64; 3] {
    let max = rgb.into_iter().fold(f64::NEG_INFINITY, f64::max);
    let min = rgb.into_iter().fold(f64::INFINITY, f64::min);
    let lightness = (max + min) / 2.0;
    let delta = max - min;
    if delta == 0.0 {
        return [0.0, 0.0, lightness];
    }
    let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
    let hue = if max == rgb[0] {
        60.0 * ((rgb[1] - rgb[2]) / delta).rem_euclid(6.0)
    } else if max == rgb[1] {
        60.0 * ((rgb[2] - rgb[0]) / delta + 2.0)
    } else {
        60.0 * ((rgb[0] - rgb[1]) / delta + 4.0)
    };
    [hue, saturation, lightness]
}

fn hsl_to_rgb([hue, saturation, lightness]: [f64; 3]) -> [f64; 3] {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let h = hue.rem_euclid(360.0) / 60.0;
    let x = chroma * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
    let pure = match h.floor() as u32 {
        0 => [chroma, x, 0.0],
        1 => [x, chroma, 0.0],
        2 => [0.0, chroma, x],
        3 => [0.0, x, chroma],
        4 => [x, 0.0, chroma],
        _ => [chroma, 0.0, x],
    };
    let offset = lightness - chroma / 2.0;
    pure.map(|value| value + offset)
}

fn adjusted_hue(reference: f64, hue: f64, method: &str) -> Option<f64> {
    let shorter = (hue - reference + 180.0).rem_euclid(360.0) - 180.0;
    let delta = match method.to_ascii_lowercase().as_str() {
        "shorter" => shorter,
        "longer" => {
            if shorter == 0.0 {
                360.0
            } else {
                shorter - shorter.signum() * 360.0
            }
        }
        "increasing" => (hue - reference).rem_euclid(360.0),
        "decreasing" => -((reference - hue).rem_euclid(360.0)),
        _ => return None,
    };
    Some(reference + delta)
}

fn canonical_math(text: &str) -> Option<String> {
    text.contains('(').then(|| math::canonical(text)).flatten()
}

fn css_number(value: f64) -> String {
    if value.is_infinite() {
        return if value.is_sign_positive() {
            "calc(infinity)"
        } else {
            "calc(-infinity)"
        }
        .into();
    }
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
    let value = if matches!(space, Space::Lch | Space::Oklch)
        && index == 1
        && text.contains('(')
        && math::evaluate(text, scale, false, context) == Some(f64::INFINITY)
    {
        Some(f64::INFINITY)
    } else {
        number(text, scale, context)?
    };
    Some(value.map(|v| match (space, index) {
        (Space::Lab | Space::Lch | Space::Oklab | Space::Oklch, 0) => v.clamp(0.0, scale),
        (Space::Lch | Space::Oklch, 1) => v.max(0.0),
        (Space::Hwb, _) => v.max(0.0) / 100.0,
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
    fn color_mix_normalizes_percentages_and_premultiplies_alpha() {
        for (input, expected, pixel) in [
            (
                "color-mix(in srgb, red, blue)",
                "color(srgb 0.5 0 0.5)",
                Color::rgb(128, 0, 128),
            ),
            (
                "color-mix(in srgb, red 20%, blue)",
                "color(srgb 0.2 0 0.8)",
                Color::rgb(51, 0, 204),
            ),
            (
                "color-mix(in srgb, red 20%, transparent 20%)",
                "color(srgb 1 0 0 / 0.2)",
                Color::rgba(255, 0, 0, 51),
            ),
        ] {
            let color = CssColor::parse(input).unwrap();
            assert_eq!(color.serialize_computed(), expected, "{input}");
            assert_eq!(color.to_color(), pixel, "{input}");
        }
    }

    #[test]
    fn color_mix_supports_hue_methods_and_rejects_invalid_percentages() {
        let shorter =
            CssColor::parse("color-mix(in hsl shorter hue, hsl(10 100% 50%), hsl(350 100% 50%))")
                .unwrap()
                .to_color();
        let longer =
            CssColor::parse("color-mix(in hsl longer hue, hsl(10 100% 50%), hsl(350 100% 50%))")
                .unwrap()
                .to_color();
        assert_eq!(shorter, Color::rgb(255, 0, 0));
        assert_eq!(longer, Color::rgb(0, 255, 255));
        for invalid in [
            "color-mix(in srgb, red)",
            "color-mix(in unknown, red, blue)",
            "color-mix(in srgb, red -1%, blue)",
            "color-mix(in srgb, red 120%, blue)",
        ] {
            assert!(CssColor::parse(invalid).is_none(), "{invalid}");
        }
    }

    #[test]
    fn font_palette_colors_mix_in_the_requested_space_and_premultiplied_alpha() {
        assert_eq!(
            mix_palette_colors(
                "in srgb",
                &[([255, 0, 0, 255], 50.0), ([0, 0, 255, 255], 50.0)]
            ),
            Some([128, 0, 128, 255])
        );
        assert_eq!(
            mix_palette_colors(
                "in srgb",
                &[([255, 0, 0, 255], 50.0), ([0, 0, 255, 0], 50.0)]
            ),
            Some([255, 0, 0, 128])
        );
        assert_eq!(
            mix_palette_colors("in srgb", &[([10, 20, 30, 255], 50.0)]),
            Some([10, 20, 30, 128])
        );
        assert_eq!(
            mix_palette_colors(
                "in hsl",
                &[([128, 128, 128, 255], 50.0), ([0, 0, 255, 255], 50.0)]
            ),
            Some([64, 64, 191, 255])
        );
    }

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
        assert_eq!(color.serialize(), "hwb(120 80% none)");
        assert_eq!(color.serialize_computed(), "hwb(120 80% none)");
        assert_eq!(
            CssColor::parse("hwb(0 0 0 / .5)").unwrap().to_color().a,
            128
        );
    }

    #[test]
    fn hwb_normalizes_unbounded_white_and_black_before_quantizing() {
        for (input, gray) in [
            ("hwb(0 200% 100%)", 170),
            ("hwb(0 100% 200%)", 85),
            ("hwb(0 150% 50%)", 191),
            ("hwb(0 50% 150%)", 64),
            ("hwb(0 180% 120%)", 153),
        ] {
            assert_eq!(
                CssColor::parse(input).unwrap().to_color(),
                Color::rgb(gray, gray, gray),
                "{input}"
            );
        }
    }

    #[test]
    fn infinite_chroma_serializes_as_math_and_converts_to_finite_rgb() {
        for (input, expected) in [
            ("lch(50 calc(infinity) 0)", "lch(50 calc(infinity) 0)"),
            ("oklch(.5 calc(infinity) 0)", "oklch(0.5 calc(infinity) 0)"),
            ("lch(50 calc(-infinity) 0)", "lch(50 0 0)"),
        ] {
            let color = CssColor::parse(input).unwrap();
            assert_eq!(color.serialize_computed(), expected);
            let rgb = convert::to_srgb(
                color.space,
                color.channels.map(|value| value.unwrap_or(0.0)),
            );
            assert!(
                rgb.into_iter()
                    .all(|channel| channel.is_finite() && (0.0..=1.0).contains(&channel))
            );
        }
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
