//! Typed edge reads preserve the general lookup path's values and fallbacks.
use super::*;

#[test]
fn typed_edge_reads_preserve_percentages_math_and_zero_bases() {
    let node = NodeHandle::element("div");
    node.set_attribute(
        "style",
        "margin:10% -2px calc(5% + 3px) 0;padding:2px 5%;border-width:1px 2px 3px 4px",
    );
    let style = StyleResolver::new().computed_style(&node);
    for basis in [None, Some(0.0), Some(200.0)] {
        for prefix in ["margin", "padding", "border", "scroll-padding"] {
            assert_eq!(
                edge_sizes_using_basis(&style, prefix, basis),
                generic_edge_sizes_using_basis(&style, prefix, basis),
                "{prefix} {basis:?}"
            );
        }
    }
    let margin = edge_sizes_with_basis(&style, "margin", 200.0);
    assert_eq!(
        [margin.top, margin.right, margin.bottom, margin.left],
        [20.0, -2.0, 13.0, 0.0]
    );
    let padding = edge_sizes_with_basis(&style, "padding", 200.0);
    assert_eq!(
        [padding.top, padding.right, padding.bottom, padding.left],
        [2.0, 10.0, 2.0, 10.0]
    );
}

#[test]
fn typed_edge_reads_preserve_legacy_aliases_and_padding_clamps() {
    let mut style = ComputedStyle::default();
    style.set_resolved_px("border-top", 7.0);
    style.set_resolved_px("border-width", 3.0);
    style.set_resolved_px("padding-top", -5.0);
    style.set_resolved_px("margin-top", -5.0);
    style.set_resolved_px("custom-right", 9.0);
    for prefix in ["margin", "padding", "border", "custom"] {
        assert_eq!(
            edge_sizes(&style, prefix),
            generic_edge_sizes_using_basis(&style, prefix, None)
        );
    }
    assert_eq!(edge_sizes(&style, "border").top, 7.0);
    assert_eq!(edge_sizes(&style, "border").right, 3.0);
    assert_eq!(edge_sizes(&style, "padding").top, 0.0);
    assert_eq!(edge_sizes(&style, "margin").top, -5.0);
    assert_eq!(edge_sizes(&style, "custom").right, 9.0);
}
