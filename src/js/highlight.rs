//! Native storage for CSS Custom Highlight API painting (Issue #1297).
//!
//! Script owns `CSS.highlights` and resolves each registered range to Text
//! node intervals. This module only publishes those intervals to the DOM so
//! layout-independent paint can read them.

use super::*;
use crate::dom::TextHighlight;

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_callable(
        context,
        bindings,
        js_string!("__omoikane_set_text_highlights"),
        2,
        NativeFunction::from_fn_ptr(set_text_highlights_native),
    )
}

/// `__omoikane_set_text_highlights(textNodeId, entries)` replaces the
/// highlight intervals of one Text node. Each entry is
/// `[start, end, name, order]` with UTF-16 offsets.
fn set_text_highlights_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    let mut highlights = Vec::new();
    if let Some(entries) = args.get(1).and_then(JsValue::as_object) {
        let length = entries
            .get(js_string!("length"), context)?
            .to_length(context)?;
        for index in 0..length {
            let entry = entries.get(index, context)?;
            let Some(entry) = entry.as_object() else {
                continue;
            };
            let number = |slot: u64, context: &mut Context| -> JsResult<f64> {
                entry.get(slot, context)?.to_number(context)
            };
            let start = number(0, context)?;
            let end = number(1, context)?;
            let name = entry
                .get(2, context)?
                .to_string(context)?
                .to_std_string_escaped();
            let order = number(3, context)?;
            if let Some(highlight) = text_highlight(start, end, &name, order) {
                highlights.push(highlight);
            }
        }
    }
    with_render_mutation(|state| {
        let node = state
            .borrow()
            .get_node(node_id)
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        if node.text_highlights() == highlights {
            return Ok(JsValue::undefined());
        }
        node.set_text_highlights(highlights);
        state.borrow_mut().invalidate_paint_cache();
        Ok(JsValue::undefined())
    })
}

/// Converts one script entry, dropping empty or non-finite intervals.
fn text_highlight(start: f64, end: f64, name: &str, order: f64) -> Option<TextHighlight> {
    if !start.is_finite() || !end.is_finite() || !order.is_finite() || start < 0.0 || end <= start {
        return None;
    }
    Some(TextHighlight {
        start: start as usize,
        end: end as usize,
        name_key: crate::css::highlight_name_key(name),
        order: order.clamp(0.0, f64::from(u32::MAX)) as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_highlight_rejects_empty_and_invalid_intervals() {
        assert!(text_highlight(2.0, 2.0, "a", 0.0).is_none());
        assert!(text_highlight(-1.0, 2.0, "a", 0.0).is_none());
        assert!(text_highlight(0.0, f64::NAN, "a", 0.0).is_none());
        let highlight = text_highlight(1.0, 4.0, "a", 3.0).unwrap();
        assert_eq!((highlight.start, highlight.end, highlight.order), (1, 4, 3));
        assert_eq!(highlight.name_key, crate::css::highlight_name_key("a"));
        assert_ne!(highlight.name_key, crate::css::highlight_name_key("A"));
    }
}
