//! Script bindings for Canvas 2D text in [`crate::canvas2d::text`]: the
//! `font` attribute parser and glyph layout with the document's fonts.

use super::*;
use crate::canvas2d::text::{self, FontContext, TextStyle};
use crate::font::{
    FontFallbackCandidate, FontFamilyKey, FontStretch, FontVariantKey, FontWeight, WebFontRegistry,
    load_default_text_fonts_shared, select_text_font,
};

/// `__omoikane_canvas_parse_font(value, context)` parses a `font` value
/// against a `{parentSize, rootSize, viewport}` JSON context. Returns the
/// [`text::CanvasFont`] as JSON, or `null` for a value the attribute ignores.
pub(super) fn parse_font_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let value = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let Some(font_context) = args
        .get(1)
        .and_then(JsValue::as_string)
        .and_then(|json| serde_json::from_str::<FontContext>(&json.to_std_string_escaped()).ok())
    else {
        return Ok(JsValue::null());
    };
    Ok(text::parse_font(&value, font_context)
        .and_then(|font| serde_json::to_string(&font).ok())
        .map_or(JsValue::null(), |json| JsString::from(json).into()))
}

/// A text layout request from script.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct TextRequest {
    text: String,
    size: f64,
    family: String,
    style: String,
    weight: String,
    stretch: String,
    #[serde(default)]
    letter_spacing: f64,
    #[serde(default)]
    word_spacing: f64,
    #[serde(default)]
    rtl: bool,
    #[serde(default = "normal_caps")]
    caps: String,
    #[serde(default = "kerning_enabled")]
    kerning: bool,
}

fn normal_caps() -> String {
    "normal".into()
}

fn kerning_enabled() -> bool {
    true
}

/// `__omoikane_canvas2d_text(canvasId, request)` lays out text with the
/// fonts of the canvas's document (web fonts, then system and default
/// fonts). `canvasId` may be `null` for a canvas without a document.
/// Returns the [`text::TextLayout`] as JSON, or `null` without fonts.
pub(super) fn text_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let canvas = match args.first() {
        Some(value) if !value.is_null_or_undefined() => Some(parse_node_id(Some(value), context)?),
        _ => None,
    };
    let Some(request) = args
        .get(1)
        .and_then(JsValue::as_string)
        .and_then(|json| serde_json::from_str::<TextRequest>(&json.to_std_string_escaped()).ok())
    else {
        return Ok(JsValue::null());
    };
    let web_fonts = canvas.and_then(|id| document_web_fonts(id).ok().flatten());
    let layout = lay_out(&request, web_fonts.as_deref());
    Ok(layout
        .and_then(|layout| serde_json::to_string(&layout).ok())
        .map_or(JsValue::null(), |json| JsString::from(json).into()))
}

/// The web fonts of the document holding node `id` (the main document for
/// a detached canvas), after its style resolver (and so its `@font-face`
/// rules) are up to date.
fn document_web_fonts(id: usize) -> JsResult<Option<Arc<WebFontRegistry>>> {
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        let document = state
            .get_node(id)
            .as_ref()
            .and_then(document_root_for_node)
            .unwrap_or_else(|| state.document.clone());
        state.ensure_style_resolver(&document);
        Ok(state
            .document_styles
            .get(&document.identity())
            .map(|entry| Arc::clone(&entry.web_fonts)))
    })
}

fn lay_out(request: &TextRequest, web_fonts: Option<&WebFontRegistry>) -> Option<text::TextLayout> {
    let family = FontFamilyKey::new(&request.family);
    let variant = FontVariantKey::from_css(
        FontWeight::parse(&request.weight),
        &request.style,
        FontStretch::parse(&request.stretch),
    );
    let defaults = load_default_text_fonts_shared();
    let web = web_fonts
        .map(|registry| registry.select_candidates_for_family_list(None, family, variant))
        .unwrap_or_default();
    let mut candidates = web
        .iter()
        .map(|candidate| FontFallbackCandidate {
            font: candidate.font,
            unicode_range: Some(candidate.unicode_range),
            weight: candidate.weight,
            style: candidate.style,
        })
        .collect::<Vec<_>>();
    let selected = select_text_font("canvas", Some(family), None, variant, web_fonts, &defaults);
    if candidates.is_empty()
        && let Some(font) = selected.as_ref()
    {
        candidates.push(FontFallbackCandidate::unrestricted(font.as_ref()));
    }
    candidates.extend(
        defaults
            .iter()
            .map(|font| FontFallbackCandidate::unrestricted(font)),
    );
    let style = TextStyle {
        size: request.size,
        letter_spacing: request.letter_spacing,
        word_spacing: request.word_spacing,
        rtl: request.rtl,
        caps: &request.caps,
        kerning: request.kerning,
    };
    text::layout_text(&candidates, &request.text, &style)
}
