//! Canvas 2D text: the `font` attribute, and text laid out as glyph
//! outlines so it fills, strokes, clips and composites like any path.

use crate::css::{CssToken, Value};
use crate::font::{
    Font, FontFallbackCandidate, OpenTypeFeature, ShapingDirection, ShapingOptions, SyntheticCaps,
    shape_text_with_fallback_candidates_and_options,
};
use serde::{Deserialize, Serialize};

/// Lengths that relative font sizes resolve against, in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FontContext {
    pub parent_size: f64,
    #[serde(default = "default_root_size")]
    pub root_size: f64,
    #[serde(default)]
    pub viewport: [f64; 2],
}

fn default_root_size() -> f64 {
    16.0
}

/// A parsed `font` value with its size computed to pixels.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct CanvasFont {
    /// The attribute value as script reads it back.
    pub serialized: String,
    pub size: f64,
    /// The family list in CSS syntax.
    pub family: String,
    pub style: String,
    pub weight: String,
    pub stretch: String,
    pub variant: String,
}

const SYSTEM_FONTS: [&str; 6] = [
    "caption",
    "icon",
    "menu",
    "message-box",
    "small-caption",
    "status-bar",
];

const GENERIC_FAMILIES: [&str; 13] = [
    "serif",
    "sans-serif",
    "cursive",
    "fantasy",
    "monospace",
    "system-ui",
    "math",
    "emoji",
    "fangsong",
    "ui-serif",
    "ui-sans-serif",
    "ui-monospace",
    "ui-rounded",
];

/// Parses a Canvas 2D `font` value as the CSS `font` shorthand, computing
/// relative sizes against `context`. `None` for values the attribute
/// ignores, including CSS-wide keywords.
pub(crate) fn parse_font(value: &str, context: FontContext) -> Option<CanvasFont> {
    let value = value.trim();
    if SYSTEM_FONTS
        .iter()
        .any(|name| value.eq_ignore_ascii_case(name))
    {
        return parse_font("13px sans-serif", context);
    }
    let (declarations, family_tokens) = crate::css::parse_canvas_font(value)?;
    let longhand = |name: &str| {
        declarations
            .iter()
            .find(|declaration| declaration.name == name)
            .map(|declaration| &declaration.value)
    };
    let size = font_size(longhand("font-size")?, context)?;
    let style = keyword(longhand("font-style")?)?;
    let variant = keyword(longhand("font-variant")?)?;
    let weight = match longhand("font-weight")? {
        Value::Number(weight) => format_number(f64::from(*weight)),
        Value::Keyword(name) => match name.as_str() {
            "normal" | "bold" => name.clone(),
            "bolder" => "bold".into(),
            "lighter" => "100".into(),
            _ => return None,
        },
        _ => return None,
    };
    let stretch = match longhand("font-stretch")? {
        Value::Percentage(percent) => stretch_keyword(f64::from(*percent)),
        Value::Keyword(name) if name != "inherit" => name.clone(),
        _ => return None,
    };
    let family = family_list(&family_tokens)?;
    let mut parts = Vec::new();
    for (part, initial) in [
        (&style, "normal"),
        (&variant, "normal"),
        (&weight, "normal"),
        (&stretch, "normal"),
    ] {
        if part != initial && !(initial == "normal" && part == "400") {
            parts.push(part.clone());
        }
    }
    parts.push(format!("{}px", format_number(size)));
    parts.push(family.clone());
    Some(CanvasFont {
        serialized: parts.join(" "),
        size,
        family,
        style,
        weight,
        stretch,
        variant,
    })
}

fn keyword(value: &Value) -> Option<String> {
    match value {
        Value::Keyword(name)
            if !matches!(
                name.as_str(),
                "inherit" | "initial" | "unset" | "revert" | "revert-layer"
            ) =>
        {
            Some(name.clone())
        }
        _ => None,
    }
}

fn stretch_keyword(percent: f64) -> String {
    const NAMES: [(f64, &str); 9] = [
        (50.0, "ultra-condensed"),
        (62.5, "extra-condensed"),
        (75.0, "condensed"),
        (87.5, "semi-condensed"),
        (100.0, "normal"),
        (112.5, "semi-expanded"),
        (125.0, "expanded"),
        (150.0, "extra-expanded"),
        (200.0, "ultra-expanded"),
    ];
    NAMES
        .iter()
        .find(|(value, _)| *value == percent)
        .map_or_else(
            || format!("{}%", format_number(percent)),
            |(_, name)| (*name).into(),
        )
}

/// Formats like the shortest JavaScript number serialization.
fn format_number(value: f64) -> String {
    let rounded = (value * 1e6).round() / 1e6;
    format!("{rounded}")
}

fn font_size(value: &Value, context: FontContext) -> Option<f64> {
    let parent = context.parent_size;
    let size = match value {
        Value::Length(number, unit) => {
            let number = f64::from(*number);
            let [width, height] = context.viewport;
            number
                * match unit.as_str() {
                    "px" => 1.0,
                    "pt" => 96.0 / 72.0,
                    "pc" => 16.0,
                    "in" => 96.0,
                    "cm" => 96.0 / 2.54,
                    "mm" => 96.0 / 25.4,
                    "q" => 96.0 / 101.6,
                    "em" | "ic" => parent,
                    "ex" | "ch" => parent / 2.0,
                    "cap" => parent * 0.7,
                    "lh" => parent * 1.2,
                    "rem" => context.root_size,
                    "rlh" => context.root_size * 1.2,
                    "vw" => width / 100.0,
                    "vh" => height / 100.0,
                    "vmin" => width.min(height) / 100.0,
                    "vmax" => width.max(height) / 100.0,
                    _ => return None,
                }
        }
        Value::Percentage(percent) => parent * f64::from(*percent) / 100.0,
        Value::Number(number) if *number == 0.0 => 0.0,
        Value::Keyword(name) => match name.as_str() {
            "xx-small" => 9.0,
            "x-small" => 10.0,
            "small" => 13.0,
            "medium" => 16.0,
            "large" => 18.0,
            "x-large" => 24.0,
            "xx-large" => 32.0,
            "xxx-large" => 48.0,
            "smaller" => parent / 1.2,
            "larger" => parent * 1.2,
            _ => return None,
        },
        _ => return None,
    };
    (size.is_finite() && size >= 0.0).then_some(size)
}

/// Serializes comma-separated family names: strings stay quoted, generic
/// names are lowercased, and CSS-wide keywords are rejected.
fn family_list(tokens: &[CssToken]) -> Option<String> {
    let mut families = Vec::new();
    for group in tokens.split(|token| matches!(token, CssToken::Comma)) {
        let parts = group
            .iter()
            .filter(|token| !matches!(token, CssToken::Whitespace))
            .collect::<Vec<_>>();
        let family = match parts.as_slice() {
            [CssToken::String(name)] => quote(name),
            [] => return None,
            idents => {
                let names = idents
                    .iter()
                    .map(|token| match token {
                        CssToken::Ident(name) => Some(name.as_str()),
                        _ => None,
                    })
                    .collect::<Option<Vec<_>>>()?;
                if let [name] = names.as_slice() {
                    let folded = name.to_ascii_lowercase();
                    if matches!(
                        folded.as_str(),
                        "inherit" | "initial" | "unset" | "revert" | "revert-layer" | "default"
                    ) {
                        return None;
                    }
                    if GENERIC_FAMILIES.contains(&folded.as_str()) {
                        families.push(folded);
                        continue;
                    }
                }
                names.join(" ")
            }
        };
        families.push(family);
    }
    Some(families.join(", "))
}

fn quote(name: &str) -> String {
    let mut quoted = String::from("\"");
    for ch in name.chars() {
        match ch {
            '"' | '\\' => {
                quoted.push('\\');
                quoted.push(ch);
            }
            '\0'..='\x1f' | '\x7f' => quoted.push_str(&format!("\\{:x} ", ch as u32)),
            _ => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}

/// Spacing and direction for [`layout_text`], in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TextStyle<'a> {
    pub size: f64,
    pub letter_spacing: f64,
    pub word_spacing: f64,
    pub rtl: bool,
    /// A `font-variant-caps` keyword.
    pub caps: &'a str,
    /// False for `fontKerning = "none"`.
    pub kerning: bool,
}

/// OpenType features and synthesis for a caps keyword and kerning choice,
/// as CSS `font-variant-caps` and `font-kerning` request them.
pub(crate) fn shaping_options(caps: &str, kerning: bool) -> ShapingOptions {
    let (tags, synthetic_caps): (&[&[u8; 4]], _) = match caps {
        "small-caps" => (&[b"smcp"], SyntheticCaps::Lowercase),
        "all-small-caps" => (&[b"c2sc", b"smcp"], SyntheticCaps::All),
        "petite-caps" => (&[b"pcap"], SyntheticCaps::Lowercase),
        "all-petite-caps" => (&[b"c2pc", b"pcap"], SyntheticCaps::All),
        "unicase" => (&[b"unic"], SyntheticCaps::None),
        "titling-caps" => (&[b"titl"], SyntheticCaps::None),
        _ => (&[], SyntheticCaps::None),
    };
    let mut features = tags
        .iter()
        .map(|tag| OpenTypeFeature::new(**tag, 1))
        .collect::<Vec<_>>();
    if !kerning {
        features.push(OpenTypeFeature::new(*b"kern", 0));
    }
    ShapingOptions {
        features,
        synthetic_caps,
        ..ShapingOptions::default()
    }
}

/// Text laid out from the start of its alphabetic baseline, y down.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TextLayout {
    /// The advance of the whole text.
    pub width: f64,
    /// Glyph outlines as closed polygons.
    pub contours: Vec<Vec<[f64; 2]>>,
    /// Ink bounds as `[left, top, right, bottom]`; `None` without ink.
    pub ink: Option<[f64; 4]>,
    pub font_ascent: f64,
    pub font_descent: f64,
    pub em_ascent: f64,
    pub em_descent: f64,
    /// Baselines above the alphabetic one (negative below).
    pub hanging: f64,
    pub ideographic: f64,
}

/// The text preparation algorithm's characters for shaping: ASCII
/// whitespace becomes a space, and U+0000, which browsers render with no
/// advance, is dropped rather than shaped with a fallback font's
/// missing-glyph box. Other control characters keep their glyphs.
fn prepare_text(text: &str) -> String {
    text.chars()
        .filter_map(|ch| {
            if ch.is_ascii_whitespace() {
                Some(' ')
            } else if ch == '\0' {
                None
            } else {
                Some(ch)
            }
        })
        .collect()
}

/// Shapes `text` with `fonts` (fallbacks in order, the first being the
/// first available font) and returns its outlines and metrics. `None`
/// without fonts or when shaping fails.
pub(crate) fn layout_text(
    fonts: &[FontFallbackCandidate<'_>],
    text: &str,
    style: &TextStyle<'_>,
) -> Option<TextLayout> {
    let primary = fonts.first()?.font;
    let text = prepare_text(text);
    let direction = if style.rtl {
        ShapingDirection::RightToLeft
    } else {
        ShapingDirection::LeftToRight
    };
    let runs = if text.is_empty() {
        Vec::new()
    } else {
        let options = shaping_options(style.caps, style.kerning);
        shape_text_with_fallback_candidates_and_options(
            fonts,
            &text,
            style.size as f32,
            direction,
            &options,
        )
        .ok()?
    };
    let mut layout = metrics(primary, style.size);
    let mut pen = 0.0;
    let mut ink = Ink::default();
    for run in &runs {
        let Some(candidate) = fonts.get(run.font_index) else {
            continue;
        };
        let units = f64::from(candidate.font.metrics().units_per_em).max(1.0);
        for glyph in &run.glyphs {
            let scale = style.size * f64::from(glyph.font_size_scale) / units;
            let origin = [pen + f64::from(glyph.x_offset), -f64::from(glyph.y_offset)];
            let tolerance = (0.1 / scale.max(1e-9)) as f32;
            for contour in candidate.font.glyph_contours(glyph.glyph_id, tolerance) {
                let points = contour
                    .iter()
                    .map(|[x, y]| {
                        let point = [
                            origin[0] + f64::from(*x) * scale,
                            origin[1] - f64::from(*y) * scale,
                        ];
                        ink.add(point);
                        point
                    })
                    .collect();
                layout.contours.push(points);
            }
            pen += f64::from(glyph.x_advance);
            if glyph.x_advance != 0.0 {
                pen += style.letter_spacing;
            }
            if text
                .get(glyph.cluster..)
                .is_some_and(|rest| rest.starts_with(' '))
            {
                pen += style.word_spacing;
            }
        }
    }
    layout.width = pen;
    layout.ink = ink.bounds;
    Some(layout)
}

/// Font-wide metrics of `font` at `size`, with no glyphs.
fn metrics(font: &Font, size: f64) -> TextLayout {
    let table = font.metrics();
    let units = f64::from(table.units_per_em).max(1.0);
    let ascent = f64::from(table.ascender) / units * size;
    let descent = -f64::from(table.descender) / units * size;
    let em_ascent = if ascent + descent > 0.0 {
        size * ascent / (ascent + descent)
    } else {
        size * 0.8
    };
    let baselines = font.horizontal_baselines();
    let scaled = |value: Option<f32>| value.map(|value| f64::from(value) / units * size);
    TextLayout {
        width: 0.0,
        contours: Vec::new(),
        ink: None,
        font_ascent: ascent,
        font_descent: descent,
        em_ascent,
        em_descent: size - em_ascent,
        hanging: scaled(baselines.hanging).unwrap_or(ascent * 0.8),
        ideographic: scaled(baselines.ideographic).unwrap_or(-descent),
    }
}

#[derive(Default)]
struct Ink {
    bounds: Option<[f64; 4]>,
}

impl Ink {
    fn add(&mut self, [x, y]: [f64; 2]) {
        let bounds = self.bounds.get_or_insert([x, y, x, y]);
        *bounds = [
            bounds[0].min(x),
            bounds[1].min(y),
            bounds[2].max(x),
            bounds[3].max(y),
        ];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTEXT: FontContext = FontContext {
        parent_size: 10.0,
        root_size: 16.0,
        viewport: [800.0, 600.0],
    };

    fn serialized(value: &str) -> Option<String> {
        parse_font(value, CONTEXT).map(|font| font.serialized)
    }

    #[test]
    fn fonts_serialize_with_computed_sizes_and_omitted_initial_values() {
        assert_eq!(serialized("20PX   SERIF").as_deref(), Some("20px serif"));
        assert_eq!(
            serialized("small-caps italic 400 12px/2 \"Unknown Font #2\", sans-serif").as_deref(),
            Some("italic small-caps 12px \"Unknown Font #2\", sans-serif")
        );
        assert_eq!(
            serialized("italic 300 12px serif").as_deref(),
            Some("italic 300 12px serif")
        );
        assert_eq!(serialized("1000% serif").as_deref(), Some("100px serif"));
        assert_eq!(
            serialized("2em Unquoted Name").as_deref(),
            Some("20px Unquoted Name")
        );
        assert_eq!(
            serialized("20px \"QuotedFont\\\\\\\",\"").as_deref(),
            Some("20px \"QuotedFont\\\\\\\",\"")
        );
        assert_eq!(
            serialized("message-box").as_deref(),
            Some("13px sans-serif")
        );
        let font = parse_font("bold condensed 12pt Mono", CONTEXT).unwrap();
        assert_eq!(font.serialized, "bold condensed 16px Mono");
        assert_eq!((font.weight.as_str(), font.size), ("bold", 16.0));
    }

    #[test]
    fn text_preparation_turns_whitespace_into_spaces_and_drops_nul() {
        // U+000B is not ASCII whitespace, so it keeps a glyph (WPT
        // 2d.text.draw.space.collapse.nonspace).
        assert_eq!(
            prepare_text("a\tb\nc\u{0}d\u{b}e\u{85}f"),
            "a b cd\u{b}e\u{85}f"
        );
    }

    #[test]
    fn caps_and_kerning_select_opentype_features_and_synthesis() {
        let options = shaping_options("all-small-caps", false);
        assert_eq!(
            options.features,
            [
                OpenTypeFeature::new(*b"c2sc", 1),
                OpenTypeFeature::new(*b"smcp", 1),
                OpenTypeFeature::new(*b"kern", 0),
            ]
        );
        assert_eq!(options.synthetic_caps, SyntheticCaps::All);
        assert_eq!(shaping_options("normal", true), ShapingOptions::default());
    }

    #[test]
    fn invalid_fonts_and_css_wide_keywords_are_rejected() {
        for value in [
            "",
            "bogus",
            "inherit",
            "10px {bogus}",
            "10px initial",
            "10px default",
            "10px inherit",
            "10px revert",
            "10px serif; color: red",
            "10px serif !important",
        ] {
            assert_eq!(serialized(value), None, "{value}");
        }
    }
}
