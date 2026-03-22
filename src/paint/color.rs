//! Color types, parsing, and gradient rendering.

use crate::layout::Rect;

use super::{blend_pixel, normalize_rect, intersect, Canvas};

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

/// A color stop in a CSS gradient.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ColorStop {
    pub(crate) color: Color,
    /// Position in the range [0.0, 1.0].
    pub(crate) position: f32,
}

/// A parsed CSS `linear-gradient()`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LinearGradient {
    /// Gradient angle in degrees (0° = to top, 90° = to right, 180° = to bottom, 270° = to left).
    pub(crate) angle_deg: f32,
    pub(crate) stops: Vec<ColorStop>,
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
    let nums: Vec<f32> = parts.iter().filter_map(|s| s.parse().ok()).collect();
    match nums.as_slice() {
        [r, g, b] => Some(Color::rgb(*r as u8, *g as u8, *b as u8)),
        [r, g, b, a] => {
            let alpha = (a.clamp(0.0, 1.0) * 255.0).round() as u8;
            Some(Color::rgba(*r as u8, *g as u8, *b as u8, alpha))
        }
        _ => None,
    }
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
                if depth > 0 {
                    depth -= 1;
                }
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

/// Parses a `linear-gradient(...)` string value into a [`LinearGradient`], or returns `None`
/// if the string is not a valid linear-gradient.
///
/// Supported direction forms:
/// - `to right`, `to left`, `to top`, `to bottom`
/// - `to top right`, `to top left`, `to bottom right`, `to bottom left`
/// - `<angle>deg` (e.g. `45deg`, `180deg`)
///
/// If no direction is given the default is `to bottom` (180°).
pub(crate) fn parse_linear_gradient(value: &str) -> Option<LinearGradient> {
    let value = value.trim();
    let args_str = value
        .strip_prefix("linear-gradient(")
        .and_then(|s| s.strip_suffix(')'))?;

    let parts = split_gradient_args(args_str);
    if parts.is_empty() {
        return None;
    }

    // Determine angle and where color stops start
    let (angle_deg, stop_start) = parse_gradient_direction(parts[0])
        .map(|a| (a, 1usize))
        .unwrap_or((180.0, 0usize));

    let stop_parts = &parts[stop_start..];
    if stop_parts.is_empty() {
        return None;
    }

    // Parse color stops; explicit positions (like `red 50%`) not yet supported — auto-space them.
    // If any stop can't be parsed as a color, treat the whole gradient as invalid.
    let mut colors = Vec::new();
    for s in stop_parts {
        match parse_color(s.trim()) {
            Some(c) => colors.push(c),
            None => return None,
        }
    }

    if colors.len() < 2 {
        return None;
    }

    let n = colors.len();
    let stops = colors
        .into_iter()
        .enumerate()
        .map(|(i, color)| {
            let position = i as f32 / (n - 1) as f32;
            ColorStop { color, position }
        })
        .collect::<Vec<_>>();

    Some(LinearGradient { angle_deg, stops })
}

/// Parses the direction part of a linear-gradient (e.g. `"to right"`, `"45deg"`).
/// Returns the angle in degrees (CSS convention: 0° = to top, 90° = to right).
pub(crate) fn parse_gradient_direction(part: &str) -> Option<f32> {
    let part = part.trim();

    // Angle: "<number>deg" (or grad/turn/rad — only deg is common)
    if let Some(deg_str) = part.strip_suffix("deg") {
        return deg_str.trim().parse::<f32>().ok();
    }
    if let Some(turn_str) = part.strip_suffix("turn") {
        return turn_str.trim().parse::<f32>().ok().map(|t| t * 360.0);
    }
    if let Some(rad_str) = part.strip_suffix("rad") {
        return rad_str
            .trim()
            .parse::<f32>()
            .ok()
            .map(|r| r.to_degrees());
    }
    if let Some(grad_str) = part.strip_suffix("grad") {
        return grad_str.trim().parse::<f32>().ok().map(|g| g * 0.9);
    }

    // Keyword: "to <side>" or "to <side> <side>"
    let lower = part.to_ascii_lowercase();
    match lower.as_str() {
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

/// Linearly interpolates a color from gradient stops at position `t` ∈ [0, 1].
pub(crate) fn interpolate_gradient_color(stops: &[ColorStop], t: f32) -> Color {
    if stops.is_empty() {
        return Color::rgba(0, 0, 0, 0);
    }
    if stops.len() == 1 || t <= stops[0].position {
        return stops[0].color;
    }
    let last = stops.last().unwrap();
    if t >= last.position {
        return last.color;
    }

    // Find the two surrounding stops
    let mut a = &stops[0];
    let mut b = &stops[1];
    for i in 0..stops.len() - 1 {
        if t >= stops[i].position && t <= stops[i + 1].position {
            a = &stops[i];
            b = &stops[i + 1];
            break;
        }
    }

    let range = b.position - a.position;
    let f = if range > 0.0 {
        (t - a.position) / range
    } else {
        0.0
    };

    Color::rgba(
        lerp_u8(a.color.r, b.color.r, f),
        lerp_u8(a.color.g, b.color.g, f),
        lerp_u8(a.color.b, b.color.b, f),
        lerp_u8(a.color.a, b.color.a, f),
    )
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round().clamp(0.0, 255.0) as u8
}

/// Draws a `LinearGradient` into the canvas over the given `area`, clipped to `clip`.
pub(crate) fn paint_linear_gradient(
    canvas: &mut Canvas,
    gradient: &LinearGradient,
    area: Rect,
    clip: Option<Rect>,
) {
    let Some(area) = normalize_rect(area) else {
        return;
    };

    // Effective clip region
    let draw_area = if let Some(clip_rect) = clip.and_then(normalize_rect) {
        match intersect(area, clip_rect) {
            Some(r) => r,
            None => return,
        }
    } else {
        area
    };

    // Convert CSS angle convention to math angle:
    // CSS 0° = "to top" (gradient goes upward, so color flows from bottom to top)
    // CSS 90° = "to right"
    // Math: angle measured counter-clockwise from positive X-axis
    //
    // For a gradient direction of angle_deg (CSS):
    //   unit vector pointing *toward* the end color:
    //   dx = sin(angle_deg), dy = -cos(angle_deg)
    //   (dy is negative because CSS Y axis is downward)
    let angle_rad = gradient.angle_deg.to_radians();
    let dir_x = angle_rad.sin();
    let dir_y = -angle_rad.cos();

    // Center of the box
    let cx = area.x + area.width * 0.5;
    let cy = area.y + area.height * 0.5;

    // Gradient length: distance from center to the "end" corner of the box
    // (CSS spec: the gradient line goes through the center and touches the side
    //  perpendicular to the gradient direction at the ending-point corner)
    // Simplified: half-length = |dx| * W/2 + |dy| * H/2
    let half_len = dir_x.abs() * area.width * 0.5 + dir_y.abs() * area.height * 0.5;
    let grad_len = half_len * 2.0;

    let x0 = area.x.floor().max(0.0) as i32;
    let y0 = area.y.floor().max(0.0) as i32;
    let x1 = (draw_area.x + draw_area.width)
        .ceil()
        .min(canvas.width as f32) as i32;
    let y1 = (draw_area.y + draw_area.height)
        .ceil()
        .min(canvas.height as f32) as i32;
    let x0 = x0.max(draw_area.x.floor() as i32);
    let y0 = y0.max(draw_area.y.floor() as i32);

    for py in y0..y1 {
        for px in x0..x1 {
            // Project pixel center onto gradient line
            let rel_x = px as f32 + 0.5 - cx;
            let rel_y = py as f32 + 0.5 - cy;
            let proj = rel_x * dir_x + rel_y * dir_y;

            // Normalize to [0, 1] along the gradient
            let t = if grad_len > 0.0 {
                (proj / grad_len + 0.5).clamp(0.0, 1.0)
            } else {
                0.0
            };

            let color = interpolate_gradient_color(&gradient.stops, t);
            let dest_index = ((py as u32 * canvas.width + px as u32) * 4) as usize;
            if dest_index + 3 < canvas.pixels.len() {
                blend_pixel(&mut canvas.pixels[dest_index..dest_index + 4], color);
            }
        }
    }
}
