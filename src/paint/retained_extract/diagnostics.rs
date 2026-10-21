//! Bounded diagnostics for retained-paint eligibility failures.
use super::*;

/// Emits the first rejected filter's numeric kernels only when requested.
/// Author CSS strings, URLs and document text never enter this diagnostic.
pub(super) fn trace_viewport_edge_filter(layout: &LayoutBox, style: &ComputedStyle, inline: bool) {
    if std::env::var_os("OMOIKANE_TRACE_RETAINED_FILTER").is_none() {
        return;
    }
    let rect = layout.dimensions.content;
    eprintln!(
        "OMOIKANE_RETAINED_FILTER {}",
        serde_json::json!({
            "node_id": layout.node.identity(),
            "tag": layout.node.tag_name().map(|tag| tag.chars().take(48).collect::<String>()),
            "raw_content": [rect.x, rect.y, rect.width, rect.height],
            "inline": inline,
            "filter": filter_kernel_diagnostic(style),
        })
    );
}

fn filter_kernel_diagnostic(style: &ComputedStyle) -> serde_json::Value {
    use crate::css::FilterFunction;
    let filters = match style.get("filter") {
        Some(ComputedValue::Keyword(value) | ComputedValue::String(value)) => {
            crate::css::parse_filter_list(value)
        }
        _ => None,
    };
    let Some(filters) = filters else {
        return serde_json::json!({"parsed": false});
    };
    let kernels = filters.iter().filter_map(|filter| match filter {
        FilterFunction::Blur(radius) => Some(serde_json::json!({"kind": "blur", "radius": radius})),
        FilterFunction::DropShadow {
            offset_x,
            offset_y,
            blur,
            color,
        } => Some(
            serde_json::json!({"kind": "drop-shadow", "offset": [offset_x, offset_y],
                "blur": blur, "color": [color.r, color.g, color.b, color.a]}),
        ),
        _ => None,
    });
    let count = kernels.clone().count();
    serde_json::json!({"parsed": true, "kernels": kernels.take(16).collect::<Vec<_>>(),
        "truncated": count > 16})
}

#[cfg(test)]
mod filter_tests {
    use super::*;

    #[test]
    fn kernel_diagnostics_preserve_zero_radius_and_bound_spatial_chains() {
        let mut style = ComputedStyle::default();
        style.set_paint_value(
            "filter",
            "brightness(.8) blur(0px) drop-shadow(1px 2px 3px red)".into(),
        );
        let value = filter_kernel_diagnostic(&style);
        assert_eq!(
            value["kernels"][0],
            serde_json::json!({"kind":"blur", "radius":0.0})
        );
        assert_eq!(value["kernels"][1]["offset"], serde_json::json!([1.0, 2.0]));
        assert_eq!(value["kernels"][1]["blur"], 3.0);
        assert_eq!(
            value["kernels"][1]["color"],
            serde_json::json!([255, 0, 0, 255])
        );
        style.set_paint_value("filter", "blur(2px) ".repeat(20));
        let value = filter_kernel_diagnostic(&style);
        assert_eq!(value["kernels"].as_array().unwrap().len(), 16);
        assert_eq!(value["truncated"], true);
    }

    #[test]
    fn unknown_filter_diagnostics_do_not_copy_author_strings() {
        let mut style = ComputedStyle::default();
        style.set_paint_value("filter", "url(https://private.example/secret)".into());
        assert_eq!(
            filter_kernel_diagnostic(&style),
            serde_json::json!({"parsed":false})
        );
    }
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct ClipAncestorDiagnostic {
    node_id: usize,
    tag: Option<String>,
    raw_padding_rect: [f32; 4],
    overflow_x: bool,
    overflow_y: bool,
    contain_paint: bool,
    clip_path: bool,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct CrossSpaceClipDiagnostic {
    node_id: usize,
    tag: Option<String>,
    ancestors: Vec<ClipAncestorDiagnostic>,
    ancestors_truncated: bool,
}

/// Returns numeric geometry and tag names, never DOM handles, text or URLs.
/// The ancestor list is bounded at 32 entries. This does not change eligibility.
pub(crate) fn diagnose_cross_space_clip(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
) -> Option<CrossSpaceClipDiagnostic> {
    find(layout, resolver, &mut Vec::new(), 0)
}

fn find(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    ancestors: &mut Vec<ClipAncestorDiagnostic>,
    clip_count: usize,
) -> Option<CrossSpaceClipDiagnostic> {
    if layout.visibility == Visibility::Hidden {
        return None;
    }
    let style = paint_box_style(layout, resolver);
    if is_fixed_for_paint(&style) {
        return (clip_count != 0).then(|| CrossSpaceClipDiagnostic {
            node_id: layout.node.identity(),
            tag: layout.node.tag_name(),
            ancestors: std::mem::take(ancestors),
            ancestors_truncated: clip_count > 32,
        });
    }
    if layout.content_visibility_contents_skipped
        || crate::layout::establishes_fixed_containing_block(&style)
    {
        return None;
    }
    let contain_paint = crate::layout::has_containment(&style, "paint");
    let clip_path = has_non_none_value(&style, "clip-path");
    let clips = layout.overflow.clips_overflow() || contain_paint || clip_path;
    let previous_len = ancestors.len();
    if clips && ancestors.len() < 32 {
        let rect = padding_box_rect(layout);
        ancestors.push(ClipAncestorDiagnostic {
            node_id: layout.node.identity(),
            tag: layout.node.tag_name(),
            raw_padding_rect: [rect.x, rect.y, rect.width, rect.height],
            overflow_x: layout.overflow.clips_x(),
            overflow_y: layout.overflow.clips_y(),
            contain_paint,
            clip_path,
        });
    }
    for child in &layout.children {
        if let Some(found) = find(child, resolver, ancestors, clip_count + usize::from(clips)) {
            return Some(found);
        }
    }
    ancestors.truncate(previous_len);
    None
}
