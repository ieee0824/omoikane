//! Color types, parsing, and gradient rendering.

use crate::layout::Rect;

use super::{Canvas, blend_pixel, intersect, normalize_rect};

/// An RGBA color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    /// Creates an opaque color.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// Creates a color with an explicit alpha channel.
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum GradientLength {
    Fraction(f32),
    Px(f32),
}

#[derive(Debug, Clone, PartialEq)]
struct GradientStop {
    color: Color,
    position: Option<GradientLength>,
    hint_before: Option<GradientLength>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PositionComponent {
    Fraction(f32),
    Px(f32),
    EndFraction(f32),
    EndPx(f32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct GradientPosition {
    x: PositionComponent,
    y: PositionComponent,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum RadialExtent {
    ClosestSide,
    FarthestSide,
    ClosestCorner,
    FarthestCorner,
    Explicit(GradientLength, Option<GradientLength>),
}

#[derive(Debug, Clone, PartialEq)]
enum GradientKind {
    Linear {
        angle_deg: f32,
    },
    Radial {
        circle: bool,
        extent: RadialExtent,
        center: GradientPosition,
    },
    Conic {
        from_deg: f32,
        center: GradientPosition,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Gradient {
    kind: GradientKind,
    stops: Vec<GradientStop>,
    repeating: bool,
}

pub(crate) fn parse_color(value: &str) -> Option<Color> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("transparent") {
        return Some(Color::rgba(0, 0, 0, 0));
    }

    let lower = value.to_ascii_lowercase();

    // Named color lookup (CSS Level 4 extended set)
    if let Some(c) = named_color(&lower) {
        return Some(c);
    }

    // Hex colors: #RGB, #RGBA, #RRGGBB, #RRGGBBAA
    if let Some(hex) = lower.strip_prefix('#') {
        if !hex.is_ascii() || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        return match hex.len() {
            3 => {
                let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
                let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
                let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
                Some(Color::rgb(r, g, b))
            }
            4 => {
                let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
                let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
                let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
                let a = u8::from_str_radix(&hex[3..4].repeat(2), 16).ok()?;
                Some(Color::rgba(r, g, b, a))
            }
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some(Color::rgb(r, g, b))
            }
            8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
                Some(Color::rgba(r, g, b, a))
            }
            _ => None,
        };
    }

    // Functional color notations handled directly for robustness
    if let Some(color) = parse_color_function(&lower) {
        return Some(color);
    }

    None
}

/// Parses `rgb()`, `rgba()`, `hsl()`, `hsla()` function notation from a string.
fn parse_color_function(value: &str) -> Option<Color> {
    let (name, args_str) = parse_function_call(value)?;

    match name {
        "rgb" | "rgba" => parse_rgb_args(args_str),
        "hsl" | "hsla" => parse_hsl_args(args_str),
        _ => None,
    }
}

/// Splits a CSS function call string into `(name, args)`.
fn parse_function_call(value: &str) -> Option<(&str, &str)> {
    let paren = value.find('(')?;
    let name = value[..paren].trim();
    if !value.ends_with(')') {
        return None;
    }
    let args = &value[paren + 1..value.len() - 1];
    Some((name, args))
}

/// Parses `rgb()` / `rgba()` argument string.
///
/// Supports both comma-separated `rgb(r, g, b)` / `rgba(r, g, b, a)` and
/// modern space-separated `rgb(r g b / a)` syntax.
fn parse_rgb_args(args: &str) -> Option<Color> {
    let parts = split_color_args(args);

    // Parse first 3 parts as RGB channels (support %)
    if parts.len() < 3 {
        return None;
    }
    let r = parse_rgb_channel(parts[0].trim())?;
    let g = parse_rgb_channel(parts[1].trim())?;
    let b = parse_rgb_channel(parts[2].trim())?;

    if parts.len() == 3 {
        return Some(Color::rgb(
            clamp_channel(r),
            clamp_channel(g),
            clamp_channel(b),
        ));
    }

    // 4th part is alpha (0-1 or percentage)
    if parts.len() >= 4 {
        let alpha = parse_alpha_value(parts[3].trim())?;
        return Some(Color::rgba(
            clamp_channel(r),
            clamp_channel(g),
            clamp_channel(b),
            (alpha * 255.0).round() as u8,
        ));
    }

    None
}

/// Parses an RGB channel value: plain number (0-255) or percentage (0%-100%).
fn parse_rgb_channel(s: &str) -> Option<f32> {
    if let Some(pct) = s.strip_suffix('%') {
        pct.parse::<f32>().ok().map(|p| p * 255.0 / 100.0)
    } else {
        s.parse().ok()
    }
}

/// Parses an alpha value: plain number (0-1) or percentage (0%-100%).
fn parse_alpha_value(s: &str) -> Option<f32> {
    if let Some(pct) = s.strip_suffix('%') {
        pct.parse::<f32>().ok().map(|p| (p / 100.0).clamp(0.0, 1.0))
    } else {
        s.parse::<f32>().ok().map(|v| v.clamp(0.0, 1.0))
    }
}

/// Clamps a color channel value to [0, 255] and rounds to u8.
fn clamp_channel(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

/// Parses `hsl()` / `hsla()` argument string.
fn parse_hsl_args(args: &str) -> Option<Color> {
    let parts = split_color_args(args);
    let nums: Vec<f32> = parts
        .iter()
        .filter_map(|s| s.trim_end_matches('%').parse().ok())
        .collect();

    match nums.as_slice() {
        [h, s, l] => {
            let (r, g, b) = hsl_to_rgb(*h, *s / 100.0, *l / 100.0);
            Some(Color::rgb(r, g, b))
        }
        [h, s, l, a] => {
            let (r, g, b) = hsl_to_rgb(*h, *s / 100.0, *l / 100.0);
            let alpha = (a.clamp(0.0, 1.0) * 255.0).round() as u8;
            Some(Color::rgba(r, g, b, alpha))
        }
        _ => None,
    }
}

/// Splits a CSS color function argument string by commas or whitespace+slash.
///
/// Handles both `255, 0, 0, 0.5` and `255 0 0 / 0.5` forms.
fn split_color_args(args: &str) -> Vec<String> {
    if args.contains(',') {
        args.split(',').map(|s| s.trim().to_string()).collect()
    } else {
        // Modern syntax: "r g b / a" — strip "/" and split by whitespace
        args.split_whitespace()
            .filter(|s| *s != "/")
            .map(|s| s.to_string())
            .collect()
    }
}

// HSL→RGB conversion is shared with src/css/style.rs
use crate::css::style::hsl_to_rgb;

/// Returns the RGB color for a CSS named color keyword.
///
/// Supports CSS Level 4 named colors (140+ colors).
#[allow(clippy::too_many_lines)]
pub(crate) fn named_color(name: &str) -> Option<Color> {
    let c = match name {
        // CSS Level 1 / basic
        "black" => Color::rgb(0, 0, 0),
        "white" => Color::rgb(255, 255, 255),
        "red" => Color::rgb(255, 0, 0),
        "green" => Color::rgb(0, 128, 0),
        "blue" => Color::rgb(0, 0, 255),
        "yellow" => Color::rgb(255, 255, 0),
        "navy" => Color::rgb(0, 0, 128),
        "purple" => Color::rgb(128, 0, 128),
        "maroon" => Color::rgb(128, 0, 0),
        "gray" | "grey" => Color::rgb(128, 128, 128),
        "silver" => Color::rgb(192, 192, 192),
        "aqua" | "cyan" => Color::rgb(0, 255, 255),
        "teal" => Color::rgb(0, 128, 128),
        "lime" => Color::rgb(0, 255, 0),
        "fuchsia" | "magenta" => Color::rgb(255, 0, 255),
        "olive" => Color::rgb(128, 128, 0),
        // Orange / red family
        "orange" => Color::rgb(255, 165, 0),
        "orangered" => Color::rgb(255, 69, 0),
        "darkorange" => Color::rgb(255, 140, 0),
        "coral" => Color::rgb(255, 127, 80),
        "tomato" => Color::rgb(255, 99, 71),
        "salmon" => Color::rgb(250, 128, 114),
        "lightsalmon" => Color::rgb(255, 160, 122),
        "darksalmon" => Color::rgb(233, 150, 122),
        "crimson" => Color::rgb(220, 20, 60),
        "firebrick" => Color::rgb(178, 34, 34),
        "darkred" => Color::rgb(139, 0, 0),
        "indianred" => Color::rgb(205, 92, 92),
        // Pink family
        "pink" => Color::rgb(255, 192, 203),
        "lightpink" => Color::rgb(255, 182, 193),
        "hotpink" => Color::rgb(255, 105, 180),
        "deeppink" => Color::rgb(255, 20, 147),
        "palevioletred" => Color::rgb(219, 112, 147),
        "mediumvioletred" => Color::rgb(199, 21, 133),
        // Gold / yellow / brown
        "gold" => Color::rgb(255, 215, 0),
        "goldenrod" => Color::rgb(218, 165, 32),
        "darkgoldenrod" => Color::rgb(184, 134, 11),
        "palegoldenrod" => Color::rgb(238, 232, 170),
        "peru" => Color::rgb(205, 133, 63),
        "chocolate" => Color::rgb(210, 105, 30),
        "sienna" => Color::rgb(160, 82, 45),
        "saddlebrown" => Color::rgb(139, 69, 19),
        "brown" => Color::rgb(165, 42, 42),
        "tan" => Color::rgb(210, 180, 140),
        "burlywood" => Color::rgb(222, 184, 135),
        "wheat" => Color::rgb(245, 222, 179),
        "sandybrown" => Color::rgb(244, 164, 96),
        "rosybrown" => Color::rgb(188, 143, 143),
        // Purple / violet
        "lavender" => Color::rgb(230, 230, 250),
        "thistle" => Color::rgb(216, 191, 216),
        "plum" => Color::rgb(221, 160, 221),
        "violet" => Color::rgb(238, 130, 238),
        "orchid" => Color::rgb(218, 112, 214),
        "mediumorchid" => Color::rgb(186, 85, 211),
        "darkorchid" => Color::rgb(153, 50, 204),
        "darkviolet" => Color::rgb(148, 0, 211),
        "blueviolet" => Color::rgb(138, 43, 226),
        "indigo" => Color::rgb(75, 0, 130),
        "slateblue" => Color::rgb(106, 90, 205),
        "darkslateblue" => Color::rgb(72, 61, 139),
        "mediumpurple" => Color::rgb(147, 112, 219),
        "rebeccapurple" => Color::rgb(102, 51, 153),
        // Blue family
        "lightblue" => Color::rgb(173, 216, 230),
        "powderblue" => Color::rgb(176, 224, 230),
        "lightskyblue" => Color::rgb(135, 206, 250),
        "skyblue" => Color::rgb(135, 206, 235),
        "deepskyblue" => Color::rgb(0, 191, 255),
        "dodgerblue" => Color::rgb(30, 144, 255),
        "cornflowerblue" => Color::rgb(100, 149, 237),
        "steelblue" => Color::rgb(70, 130, 180),
        "royalblue" => Color::rgb(65, 105, 225),
        "mediumblue" => Color::rgb(0, 0, 205),
        "darkblue" => Color::rgb(0, 0, 139),
        "midnightblue" => Color::rgb(25, 25, 112),
        "azure" => Color::rgb(240, 255, 255),
        "aliceblue" => Color::rgb(240, 248, 255),
        "ghostwhite" => Color::rgb(248, 248, 255),
        "lavenderblush" => Color::rgb(255, 240, 245),
        // Green family
        "mintcream" => Color::rgb(245, 255, 250),
        "honeydew" => Color::rgb(240, 255, 240),
        "lightgreen" => Color::rgb(144, 238, 144),
        "palegreen" => Color::rgb(152, 251, 152),
        "limegreen" => Color::rgb(50, 205, 50),
        "mediumseagreen" => Color::rgb(60, 179, 113),
        "seagreen" => Color::rgb(46, 139, 87),
        "forestgreen" => Color::rgb(34, 139, 34),
        "darkgreen" => Color::rgb(0, 100, 0),
        "yellowgreen" => Color::rgb(154, 205, 50),
        "olivedrab" => Color::rgb(107, 142, 35),
        "darkolivegreen" => Color::rgb(85, 107, 47),
        "mediumaquamarine" => Color::rgb(102, 205, 170),
        "aquamarine" => Color::rgb(127, 255, 212),
        "turquoise" => Color::rgb(64, 224, 208),
        "mediumturquoise" => Color::rgb(72, 209, 204),
        "darkturquoise" => Color::rgb(0, 206, 209),
        "lightseagreen" => Color::rgb(32, 178, 170),
        "cadetblue" => Color::rgb(95, 158, 160),
        "darkcyan" => Color::rgb(0, 139, 139),
        "darkslategray" | "darkslategrey" => Color::rgb(47, 79, 79),
        "slategray" | "slategrey" => Color::rgb(112, 128, 144),
        "lightslategray" | "lightslategrey" => Color::rgb(119, 136, 153),
        // Gray shades
        "darkgray" | "darkgrey" => Color::rgb(169, 169, 169),
        "dimgray" | "dimgrey" => Color::rgb(105, 105, 105),
        "lightgray" | "lightgrey" => Color::rgb(211, 211, 211),
        "gainsboro" => Color::rgb(220, 220, 220),
        "whitesmoke" => Color::rgb(245, 245, 245),
        "snow" => Color::rgb(255, 250, 250),
        "seashell" => Color::rgb(255, 245, 238),
        "floralwhite" => Color::rgb(255, 250, 240),
        "ivory" => Color::rgb(255, 255, 240),
        "linen" => Color::rgb(250, 240, 230),
        "oldlace" => Color::rgb(253, 245, 230),
        "antiquewhite" => Color::rgb(250, 235, 215),
        "bisque" => Color::rgb(255, 228, 196),
        "blanchedalmond" => Color::rgb(255, 235, 205),
        "moccasin" => Color::rgb(255, 228, 181),
        "navajowhite" => Color::rgb(255, 222, 173),
        "peachpuff" => Color::rgb(255, 218, 185),
        "mistyrose" => Color::rgb(255, 228, 225),
        "papayawhip" => Color::rgb(255, 239, 213),
        "lightyellow" => Color::rgb(255, 255, 224),
        "lemonchiffon" => Color::rgb(255, 250, 205),
        "cornsilk" => Color::rgb(255, 248, 220),
        "beige" => Color::rgb(245, 245, 220),
        "khaki" => Color::rgb(240, 230, 140),
        "darkkhaki" => Color::rgb(189, 183, 107),
        // Chartreuse / spring
        "chartreuse" => Color::rgb(127, 255, 0),
        "lawngreen" => Color::rgb(124, 252, 0),
        "greenyellow" => Color::rgb(173, 255, 47),
        "springgreen" => Color::rgb(0, 255, 127),
        "mediumslateblue" => Color::rgb(123, 104, 238),
        "mediumspringgreen" => Color::rgb(0, 250, 154),
        // Missing CSS Level 4 colors
        "darkmagenta" => Color::rgb(139, 0, 139),
        "darkseagreen" => Color::rgb(143, 188, 143),
        "lightcoral" => Color::rgb(240, 128, 128),
        "lightcyan" => Color::rgb(224, 255, 255),
        "lightgoldenrodyellow" => Color::rgb(250, 250, 210),
        "lightsteelblue" => Color::rgb(176, 196, 222),
        "paleturquoise" => Color::rgb(175, 238, 238),
        _ => return None,
    };
    Some(c)
}

/// Splits gradient arguments at the top-level commas (skipping nested parens).
pub(crate) fn split_gradient_args(args: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth: usize = 0;
    let mut start = 0;
    for (i, ch) in args.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
            }
            ',' if depth == 0 => {
                parts.push(args[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(args[start..].trim());
    parts
}

/// Parses the direction part of a linear-gradient (e.g. `"to right"`, `"45deg"`).
/// Returns the angle in degrees (CSS convention: 0° = to top, 90° = to right).
pub(crate) fn parse_gradient_direction(part: &str) -> Option<f32> {
    let lower = part.trim().to_ascii_lowercase();
    let part = lower.as_str();
    let finite = |value: &str| value.trim().parse::<f32>().ok().filter(|v| v.is_finite());

    // Angle: "<number>deg" (or grad/turn/rad — only deg is common)
    if let Some(deg_str) = part.strip_suffix("deg") {
        return finite(deg_str);
    }
    if let Some(turn_str) = part.strip_suffix("turn") {
        return finite(turn_str)
            .map(|t| t * 360.0)
            .filter(|v| v.is_finite());
    }
    if let Some(rad_str) = part.strip_suffix("rad") {
        return finite(rad_str)
            .map(|r| r.to_degrees())
            .filter(|v| v.is_finite());
    }
    if let Some(grad_str) = part.strip_suffix("grad") {
        return finite(grad_str).map(|g| g * 0.9).filter(|v| v.is_finite());
    }

    // Keyword: "to <side>" or "to <side> <side>"
    match part {
        "to top" => Some(0.0),
        "to right" => Some(90.0),
        "to bottom" => Some(180.0),
        "to left" => Some(270.0),
        "to top right" | "to right top" => Some(45.0),
        "to bottom right" | "to right bottom" => Some(135.0),
        "to bottom left" | "to left bottom" => Some(225.0),
        "to top left" | "to left top" => Some(315.0),
        _ => None,
    }
}

fn default_center() -> GradientPosition {
    GradientPosition {
        x: PositionComponent::Fraction(0.5),
        y: PositionComponent::Fraction(0.5),
    }
}

fn parse_number_with_unit(value: &str) -> Option<GradientLength> {
    let value = value.trim().to_ascii_lowercase();
    if let Some(number) = value.strip_suffix('%') {
        return number
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
            .map(|v| GradientLength::Fraction(v / 100.0));
    }
    if let Some(number) = value.strip_suffix("px") {
        return number
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
            .map(GradientLength::Px);
    }
    if value == "0" {
        return Some(GradientLength::Px(0.0));
    }
    None
}

fn parse_angle(value: &str) -> Option<f32> {
    let value = value.trim().to_ascii_lowercase();
    parse_gradient_direction(&value).or_else(|| {
        let value = value.as_str();
        (value == "0").then_some(0.0)
    })
}

fn top_level_words(value: &str) -> Vec<&str> {
    let mut words = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    for (index, ch) in value.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            c if c.is_whitespace() && depth == 0 => {
                if let Some(begin) = start.take() {
                    words.push(value[begin..index].trim());
                }
            }
            _ if start.is_none() => start = Some(index),
            _ => {}
        }
    }
    if let Some(begin) = start {
        words.push(value[begin..].trim());
    }
    words
}

fn parse_position(value: &str) -> Option<GradientPosition> {
    let words = top_level_words(value);
    if words.is_empty() || words.len() > 4 {
        return None;
    }

    let plain = |word: &str, horizontal: bool| match word.to_ascii_lowercase().as_str() {
        "center" => Some(PositionComponent::Fraction(0.5)),
        "left" if horizontal => Some(PositionComponent::Fraction(0.0)),
        "right" if horizontal => Some(PositionComponent::Fraction(1.0)),
        "top" if !horizontal => Some(PositionComponent::Fraction(0.0)),
        "bottom" if !horizontal => Some(PositionComponent::Fraction(1.0)),
        _ => match parse_number_with_unit(word)? {
            GradientLength::Fraction(v) => Some(PositionComponent::Fraction(v)),
            GradientLength::Px(v) => Some(PositionComponent::Px(v)),
        },
    };
    let axis = |component: &[&str], horizontal: bool| -> Option<PositionComponent> {
        match component {
            [word] => plain(word, horizontal),
            [edge, offset] => {
                let from_end = match (edge.to_ascii_lowercase().as_str(), horizontal) {
                    ("left", true) | ("top", false) => false,
                    ("right", true) | ("bottom", false) => true,
                    _ => return None,
                };
                match (parse_number_with_unit(offset)?, from_end) {
                    (GradientLength::Fraction(value), false) => {
                        Some(PositionComponent::Fraction(value))
                    }
                    (GradientLength::Px(value), false) => Some(PositionComponent::Px(value)),
                    (GradientLength::Fraction(value), true) => {
                        Some(PositionComponent::EndFraction(value))
                    }
                    (GradientLength::Px(value), true) => Some(PositionComponent::EndPx(value)),
                }
            }
            _ => None,
        }
    };
    if words.len() == 1 {
        return match words[0].to_ascii_lowercase().as_str() {
            "top" | "bottom" => Some(GradientPosition {
                x: PositionComponent::Fraction(0.5),
                y: plain(words[0], false)?,
            }),
            _ => Some(GradientPosition {
                x: plain(words[0], true)?,
                y: PositionComponent::Fraction(0.5),
            }),
        };
    }

    // A position consists of one horizontal and one vertical component. Each
    // component is either one token or an edge keyword plus its offset.
    for split in 1..words.len() {
        let first = &words[..split];
        let second = &words[split..];
        if let (Some(x), Some(y)) = (axis(first, true), axis(second, false)) {
            return Some(GradientPosition { x, y });
        }
        if let (Some(y), Some(x)) = (axis(first, false), axis(second, true)) {
            return Some(GradientPosition { x, y });
        }
    }
    None
}

fn parse_color_stop(value: &str, conic: bool) -> Option<Vec<GradientStop>> {
    let words = top_level_words(value);
    if words.is_empty() {
        return None;
    }
    let color = parse_color(words[0])?;
    if words.len() > 3 {
        return None;
    }
    let parse_pos = |word: &str| -> Option<GradientLength> {
        if conic {
            parse_angle(word)
                .map(|v| GradientLength::Fraction(v / 360.0))
                .or_else(|| parse_number_with_unit(word))
        } else {
            parse_number_with_unit(word)
        }
    };
    let first = match words.get(1) {
        Some(word) => Some(parse_pos(word)?),
        None => None,
    };
    let second = match words.get(2) {
        Some(word) => Some(parse_pos(word)?),
        None => None,
    };
    let mut result = vec![GradientStop {
        color,
        position: first,
        hint_before: None,
    }];
    if let Some(position) = second {
        result.push(GradientStop {
            color,
            position: Some(position),
            hint_before: None,
        });
    }
    Some(result)
}

fn parse_stops(parts: &[&str], conic: bool) -> Option<Vec<GradientStop>> {
    let mut stops = Vec::new();
    let mut pending_hint = None;
    for part in parts {
        if let Some(mut parsed) = parse_color_stop(part, conic) {
            if let Some(hint) = pending_hint.take() {
                parsed[0].hint_before = Some(hint);
            }
            stops.extend(parsed);
        } else {
            let hint = if conic {
                parse_angle(part)
                    .map(|v| GradientLength::Fraction(v / 360.0))
                    .or_else(|| parse_number_with_unit(part))
            } else {
                parse_number_with_unit(part)
            }?;
            if stops.is_empty() || pending_hint.replace(hint).is_some() {
                return None;
            }
        }
    }
    if pending_hint.is_some() || stops.len() < 2 {
        None
    } else {
        Some(stops)
    }
}

pub(crate) fn parse_gradient(value: &str) -> Option<Gradient> {
    let value = value.trim();
    let open = value.find('(')?;
    if !value.ends_with(')') {
        return None;
    }
    let name = value[..open].trim().to_ascii_lowercase();
    let repeating = name.starts_with("repeating-");
    let base = name.strip_prefix("repeating-").unwrap_or(&name);
    if !matches!(
        base,
        "linear-gradient" | "radial-gradient" | "conic-gradient"
    ) {
        return None;
    }
    let parts = split_gradient_args(&value[open + 1..value.len() - 1]);
    if parts.iter().any(|part| part.is_empty()) {
        return None;
    }

    if base == "linear-gradient" {
        let (angle_deg, start) = parse_gradient_direction(parts[0])
            .map(|v| (v, 1))
            .unwrap_or((180.0, 0));
        return Some(Gradient {
            kind: GradientKind::Linear { angle_deg },
            stops: parse_stops(&parts[start..], false)?,
            repeating,
        });
    }

    if base == "conic-gradient" {
        let words = top_level_words(parts[0]);
        let mut from_deg = 0.0;
        let mut center = default_center();
        let mut index = 0usize;
        let mut has_prelude = false;
        let mut has_from = false;
        while index < words.len() {
            match words[index].to_ascii_lowercase().as_str() {
                "from" if index + 1 < words.len() && !has_from => {
                    from_deg = parse_angle(words[index + 1])?;
                    index += 2;
                    has_prelude = true;
                    has_from = true;
                }
                "at" if index + 1 < words.len() => {
                    center = parse_position(&words[index + 1..].join(" "))?;
                    index = words.len();
                    has_prelude = true;
                }
                _ => break,
            }
        }
        if has_prelude && index != words.len() {
            return None;
        }
        let start = usize::from(has_prelude);
        return Some(Gradient {
            kind: GradientKind::Conic { from_deg, center },
            stops: parse_stops(&parts[start..], true)?,
            repeating,
        });
    }

    let words = top_level_words(parts[0]);
    let looks_like_prelude = words.iter().any(|word| {
        matches!(
            word.to_ascii_lowercase().as_str(),
            "circle"
                | "ellipse"
                | "closest-side"
                | "farthest-side"
                | "closest-corner"
                | "farthest-corner"
                | "at"
        )
    }) || words
        .first()
        .and_then(|word| parse_number_with_unit(word))
        .is_some();
    let mut circle = false;
    let mut shape_set = false;
    let mut extent = RadialExtent::FarthestCorner;
    let mut center = default_center();
    if looks_like_prelude {
        let at = words
            .iter()
            .position(|word| word.eq_ignore_ascii_case("at"));
        let geometry = &words[..at.unwrap_or(words.len())];
        if let Some(at) = at {
            center = parse_position(&words[at + 1..].join(" "))?;
        }
        let mut lengths = Vec::new();
        let mut extent_set = false;
        for word in geometry {
            match word.to_ascii_lowercase().as_str() {
                "circle" => {
                    if shape_set {
                        return None;
                    }
                    circle = true;
                    shape_set = true;
                }
                "ellipse" => {
                    if shape_set {
                        return None;
                    }
                    circle = false;
                    shape_set = true;
                }
                "closest-side" if !extent_set => {
                    extent = RadialExtent::ClosestSide;
                    extent_set = true;
                }
                "farthest-side" if !extent_set => {
                    extent = RadialExtent::FarthestSide;
                    extent_set = true;
                }
                "closest-corner" if !extent_set => {
                    extent = RadialExtent::ClosestCorner;
                    extent_set = true;
                }
                "farthest-corner" if !extent_set => {
                    extent = RadialExtent::FarthestCorner;
                    extent_set = true;
                }
                _ => lengths.push(parse_number_with_unit(word)?),
            }
        }
        if !lengths.is_empty() {
            if extent_set
                || lengths.len() > 2
                || lengths.iter().any(|length| matches!(length, GradientLength::Px(v) | GradientLength::Fraction(v) if *v < 0.0))
            {
                return None;
            }
            if circle && lengths.len() != 1 {
                return None;
            }
            if !shape_set && lengths.len() == 1 {
                circle = true;
            }
            if circle
                && lengths
                    .iter()
                    .any(|length| matches!(length, GradientLength::Fraction(_)))
            {
                return None;
            }
            extent = RadialExtent::Explicit(lengths[0], lengths.get(1).copied());
        }
    }
    Some(Gradient {
        kind: GradientKind::Radial {
            circle,
            extent,
            center,
        },
        stops: parse_stops(&parts[usize::from(looks_like_prelude)..], false)?,
        repeating,
    })
}

fn resolve_component(component: PositionComponent, origin: f32, size: f32) -> f32 {
    origin
        + match component {
            PositionComponent::Fraction(v) => v * size,
            PositionComponent::Px(v) => v,
            PositionComponent::EndFraction(v) => (1.0 - v) * size,
            PositionComponent::EndPx(v) => size - v,
        }
}

fn resolve_length(length: GradientLength, basis: f32) -> f32 {
    match length {
        GradientLength::Fraction(v) => v * basis,
        GradientLength::Px(v) => v,
    }
}

fn resolved_stops(stops: &[GradientStop], basis: f32) -> Vec<(Color, f32, Option<f32>)> {
    let mut stop_positions: Vec<Option<f32>> = stops
        .iter()
        .map(|stop| stop.position.map(|v| resolve_length(v, basis)))
        .collect();
    if stop_positions[0].is_none() {
        stop_positions[0] = Some(0.0);
    }
    if stop_positions.last().is_some_and(Option::is_none) {
        *stop_positions.last_mut().unwrap() = Some(basis);
    }

    // CSS Images 3 §3.4.3 fixes color stops and transition hints in their
    // specified order. A hint therefore participates in monotonic fixup and
    // can push the following color stop forward.
    let mut sequence = Vec::with_capacity(stops.len() * 2 - 1);
    for (index, stop) in stops.iter().enumerate() {
        if index > 0
            && let Some(hint) = stop.hint_before
        {
            sequence.push((index, true, Some(resolve_length(hint, basis))));
        }
        sequence.push((index, false, stop_positions[index]));
    }

    let mut last = sequence[0].2.unwrap();
    for (_, _, position) in sequence.iter_mut().skip(1) {
        if let Some(value) = position {
            *value = value.max(last);
            last = *value;
        }
    }

    // Distribute unspecified stops between the nearest positioned items in
    // the same combined sequence, so a preceding hint remains an ordering
    // boundary for an otherwise automatic stop.
    let mut index = 1;
    while index + 1 < sequence.len() {
        if sequence[index].2.is_some() {
            index += 1;
            continue;
        }
        let start = index - 1;
        let mut end = index + 1;
        while sequence[end].2.is_none() {
            end += 1;
        }
        let a = sequence[start].2.unwrap();
        let b = sequence[end].2.unwrap();
        for current in index..end {
            sequence[current].2 =
                Some(a + (b - a) * (current - start) as f32 / (end - start) as f32);
        }
        index = end;
    }

    let mut resolved = stops
        .iter()
        .map(|stop| (stop.color, 0.0, None))
        .collect::<Vec<_>>();
    for (stop_index, is_hint, position) in sequence {
        if is_hint {
            resolved[stop_index].2 = position;
        } else {
            resolved[stop_index].1 = position.unwrap();
        }
    }
    resolved
}

fn sample_stops(stops: &[(Color, f32, Option<f32>)], mut value: f32, repeating: bool) -> Color {
    let first = stops[0].1;
    let last = stops.last().unwrap().1;
    if repeating {
        let period = last - first;
        if period <= f32::EPSILON {
            return stops.last().unwrap().0;
        }
        value = (value - first).rem_euclid(period) + first;
    }
    if value <= first {
        return stops[0].0;
    }
    if value >= last {
        return stops.last().unwrap().0;
    }
    let upper = stops
        .partition_point(|(_, position, _)| *position <= value)
        .min(stops.len() - 1);
    let (a_color, a_pos, _) = stops[upper - 1];
    let (b_color, b_pos, hint) = stops[upper];
    if b_pos <= a_pos {
        return b_color;
    }
    let mut t = (value - a_pos) / (b_pos - a_pos);
    if let Some(hint) = hint {
        let midpoint = ((hint - a_pos) / (b_pos - a_pos)).clamp(0.0, 1.0);
        t = if (midpoint - 0.5).abs() <= f32::EPSILON {
            t
        } else if midpoint <= f32::EPSILON {
            if t <= 0.0 { 0.0 } else { 1.0 }
        } else if midpoint >= 1.0 - f32::EPSILON {
            if t >= 1.0 { 1.0 } else { 0.0 }
        } else {
            let exponent = 0.5_f32.ln() / midpoint.ln();
            t.clamp(0.0, 1.0).powf(exponent)
        };
    }
    // CSS gradients interpolate in premultiplied-alpha space.
    let aa = a_color.a as f32 / 255.0;
    let ba = b_color.a as f32 / 255.0;
    let alpha = aa + (ba - aa) * t;
    let channel = |a: u8, b: u8| {
        if alpha <= f32::EPSILON {
            0
        } else {
            (((a as f32 * aa) + (b as f32 * ba - a as f32 * aa) * t) / alpha)
                .round()
                .clamp(0.0, 255.0) as u8
        }
    };
    Color::rgba(
        channel(a_color.r, b_color.r),
        channel(a_color.g, b_color.g),
        channel(a_color.b, b_color.b),
        (alpha * 255.0).round() as u8,
    )
}

pub(crate) fn paint_gradient(
    canvas: &mut Canvas,
    gradient: &Gradient,
    area: Rect,
    clip: Option<Rect>,
) {
    let Some(area) = normalize_rect(area) else {
        return;
    };
    let draw_area = clip
        .and_then(normalize_rect)
        .map(|clip| intersect(area, clip))
        .unwrap_or(Some(area));
    let Some(draw_area) = draw_area else {
        return;
    };
    let (center_x, center_y, radius_x, radius_y, basis) = match gradient.kind {
        GradientKind::Linear { angle_deg } => {
            let angle = angle_deg.to_radians();
            let dx = angle.sin();
            let dy = -angle.cos();
            let length = dx.abs() * area.width + dy.abs() * area.height;
            (
                area.x + area.width / 2.0,
                area.y + area.height / 2.0,
                dx,
                dy,
                length,
            )
        }
        GradientKind::Radial {
            circle,
            extent,
            center,
        } => {
            let cx = resolve_component(center.x, area.x, area.width);
            let cy = resolve_component(center.y, area.y, area.height);
            let left = (cx - area.x).abs();
            let right = (area.x + area.width - cx).abs();
            let top = (cy - area.y).abs();
            let bottom = (area.y + area.height - cy).abs();
            let (mut rx, mut ry) = match extent {
                RadialExtent::ClosestSide => (left.min(right), top.min(bottom)),
                RadialExtent::FarthestSide => (left.max(right), top.max(bottom)),
                RadialExtent::ClosestCorner | RadialExtent::FarthestCorner => {
                    let choose = if matches!(extent, RadialExtent::ClosestCorner) {
                        f32::min
                    } else {
                        f32::max
                    };
                    let x = choose(left, right);
                    let y = choose(top, bottom);
                    (x, y)
                }
                RadialExtent::Explicit(x, y) => (
                    resolve_length(x, area.width),
                    resolve_length(y.unwrap_or(x), area.height),
                ),
            };
            if circle {
                let radius = match extent {
                    RadialExtent::ClosestSide => left.min(right).min(top.min(bottom)),
                    RadialExtent::FarthestSide => left.max(right).max(top.max(bottom)),
                    RadialExtent::ClosestCorner => {
                        [(left, top), (left, bottom), (right, top), (right, bottom)]
                            .into_iter()
                            .map(|(x, y)| x.hypot(y))
                            .fold(f32::INFINITY, f32::min)
                    }
                    RadialExtent::FarthestCorner => {
                        [(left, top), (left, bottom), (right, top), (right, bottom)]
                            .into_iter()
                            .map(|(x, y)| x.hypot(y))
                            .fold(0.0, f32::max)
                    }
                    RadialExtent::Explicit(x, _) => resolve_length(x, area.width.min(area.height)),
                };
                rx = radius;
                ry = radius;
            } else if matches!(
                extent,
                RadialExtent::ClosestCorner | RadialExtent::FarthestCorner
            ) {
                // Scale the side-based ellipse so the selected corner lies on it.
                let scale = 2.0_f32.sqrt();
                rx *= scale;
                ry *= scale;
            }
            (cx, cy, rx, ry, rx.max(f32::EPSILON))
        }
        GradientKind::Conic { center, .. } => (
            resolve_component(center.x, area.x, area.width),
            resolve_component(center.y, area.y, area.height),
            0.0,
            0.0,
            1.0,
        ),
    };
    let stops = resolved_stops(&gradient.stops, basis);
    let x0 = draw_area.x.floor().max(0.0) as i32;
    let y0 = draw_area.y.floor().max(0.0) as i32;
    let x1 = (draw_area.x + draw_area.width)
        .ceil()
        .min(canvas.width as f32) as i32;
    let y1 = (draw_area.y + draw_area.height)
        .ceil()
        .min(canvas.height as f32) as i32;
    for py in y0..y1 {
        for px in x0..x1 {
            let x = px as f32 + 0.5 - center_x;
            let y = py as f32 + 0.5 - center_y;
            let value = match gradient.kind {
                GradientKind::Linear { .. } => x * radius_x + y * radius_y + basis / 2.0,
                GradientKind::Radial { .. } => {
                    if radius_x <= 0.0 || radius_y <= 0.0 {
                        basis
                    } else {
                        ((x / radius_x).powi(2) + (y / radius_y).powi(2)).sqrt() * basis
                    }
                }
                GradientKind::Conic { from_deg, .. } => {
                    ((x.atan2(-y).to_degrees() - from_deg).rem_euclid(360.0)) / 360.0
                }
            };
            let color = sample_stops(&stops, value, gradient.repeating);
            let index = ((py as u32 * canvas.width + px as u32) * 4) as usize;
            if index + 3 < canvas.pixels.len() {
                blend_pixel(&mut canvas.pixels[index..index + 4], color);
            }
        }
    }
}

#[cfg(test)]
mod gradient_tests {
    use super::*;

    #[test]
    fn color_hint_uses_the_css_exponential_midpoint_curve() {
        let stops = [
            (Color::rgb(255, 0, 0), 0.0, None),
            (Color::rgb(0, 0, 255), 1.0, Some(0.25)),
        ];
        let eighth = sample_stops(&stops, 0.125, false);
        let hint = sample_stops(&stops, 0.25, false);
        let half = sample_stops(&stops, 0.5, false);

        assert_eq!(eighth, Color::rgb(165, 0, 90));
        assert_eq!(hint, Color::rgb(128, 0, 128));
        assert_eq!(half, Color::rgb(75, 0, 180));

        let linear = [stops[0], (stops[1].0, stops[1].1, Some(0.5))];
        assert_eq!(sample_stops(&linear, 0.125, false), Color::rgb(223, 0, 32));
        let at_start = [stops[0], (stops[1].0, stops[1].1, Some(0.0))];
        assert_eq!(sample_stops(&at_start, 0.125, false), Color::rgb(0, 0, 255));
        let at_end = [stops[0], (stops[1].0, stops[1].1, Some(1.0))];
        assert_eq!(sample_stops(&at_end, 0.5, false), Color::rgb(255, 0, 0));
    }

    #[test]
    fn color_stop_fixup_orders_hints_with_their_surrounding_stops() {
        let resolve = |value: &str| {
            let gradient = parse_gradient(value).unwrap();
            resolved_stops(&gradient.stops, 1.0)
        };

        let before_prior = resolve("linear-gradient(red 50%, 20%, blue 100%)");
        assert_eq!(before_prior[0].1, 0.5);
        assert_eq!(before_prior[1].2, Some(0.5));
        assert_eq!(before_prior[1].1, 1.0);

        let beyond_following = resolve("linear-gradient(red 0%, 120%, blue 100%)");
        assert_eq!(beyond_following[0].1, 0.0);
        assert_eq!(beyond_following[1].2, Some(1.2));
        assert_eq!(beyond_following[1].1, 1.2);

        let normal = resolve("linear-gradient(red 0%, 25%, blue 100%)");
        assert_eq!(normal[0].1, 0.0);
        assert_eq!(normal[1].2, Some(0.25));
        assert_eq!(normal[1].1, 1.0);
    }

    #[test]
    fn radial_and_conic_positions_accept_edge_offsets() {
        let radial =
            parse_gradient("radial-gradient(circle 5px at right 10px bottom 20px, red, blue)")
                .unwrap();
        let GradientKind::Radial { center, .. } = radial.kind else {
            panic!("expected radial gradient");
        };
        assert_eq!(resolve_component(center.x, 0.0, 100.0), 90.0);
        assert_eq!(resolve_component(center.y, 0.0, 80.0), 60.0);

        let conic = parse_gradient("conic-gradient(at bottom 25% right 10%, red, blue)").unwrap();
        let GradientKind::Conic { center, .. } = conic.kind else {
            panic!("expected conic gradient");
        };
        assert_eq!(resolve_component(center.x, 0.0, 100.0), 90.0);
        assert_eq!(resolve_component(center.y, 0.0, 80.0), 60.0);

        assert!(parse_gradient("conic-gradient(at center right 10px, red, blue)").is_some());
        assert!(parse_gradient("radial-gradient(at right 10px center, red, blue)").is_some());
    }

    #[test]
    fn malformed_positions_and_duplicate_conic_preludes_are_rejected() {
        for value in [
            "radial-gradient(at left right, red, blue)",
            "radial-gradient(at top 1px bottom 2px, red, blue)",
            "conic-gradient(from 10deg from 20deg, red, blue)",
            "conic-gradient(at left at top, red, blue)",
            "conic-gradient(at right 10px left 20px, red, blue)",
        ] {
            assert!(parse_gradient(value).is_none(), "accepted {value}");
        }
    }

    #[test]
    fn non_finite_numbers_and_negative_radii_are_rejected_but_zero_is_valid() {
        for value in [
            "linear-gradient(NaNdeg, red, blue)",
            "conic-gradient(from infdeg, red, blue)",
            "radial-gradient(circle NaNpx, red, blue)",
            "radial-gradient(ellipse infpx 2px, red, blue)",
            "radial-gradient(circle -1px, red, blue)",
            "radial-gradient(ellipse 1px -1px, red, blue)",
            "radial-gradient(at NaNpx center, red, blue)",
        ] {
            assert!(parse_gradient(value).is_none(), "accepted {value}");
        }
        assert!(parse_gradient("radial-gradient(circle 0px, red, blue)").is_some());
        assert!(parse_gradient("radial-gradient(ellipse 0px 0px, red, blue)").is_some());
    }
}
