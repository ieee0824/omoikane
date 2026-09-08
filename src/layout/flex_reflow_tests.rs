//! Flex item percentage-height regressions, with dimensions checked against
//! Firefox and CSS Flexbox sections 9.4 and 9.8.

use super::*;
use crate::css::{Origin, parse_stylesheet};

fn percentage_descendants(
    container_style: &str,
    item_style: &str,
    tall_sibling: bool,
) -> (LayoutBox, ComputedStyle) {
    let body = NodeHandle::element("body");
    let container = NodeHandle::element("main");
    let item = NodeHandle::element("article");
    let child = NodeHandle::element("section");
    child.append_child(NodeHandle::element("div"));
    item.append_child(child);
    container.append_child(item.clone());
    if tall_sibling {
        container.append_child(NodeHandle::element("aside"));
    }
    body.append_child(container);
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(&format!(
            "body {{ margin: 0; }} \
             main {{ display: flex; width: 300px; {container_style} }} \
             article {{ {item_style} }} \
             section {{ height: 100%; }} div {{ height: 50%; }} \
             aside {{ width: 20px; height: 120px; }}"
        ))
        .unwrap(),
    );
    let original_style = resolver.computed_style(&item);
    let mut layout = layout_tree(
        &body,
        &mut resolver,
        Rect {
            width: 300.0,
            ..Rect::default()
        },
    )
    .unwrap();
    assert_eq!(resolver.computed_style(&item), original_style);
    (layout.children.remove(0), original_style)
}

fn assert_descendant_heights(container: &LayoutBox, item_height: f32, child_height: f32) {
    let item = &container.children[0];
    let child = &item.children[0];
    let grandchild = &child.children[0];
    assert_eq!(item.dimensions.content.height, item_height, "flex item");
    assert_eq!(child.dimensions.content.height, child_height, "100% child");
    assert_eq!(
        grandchild.dimensions.content.height,
        child_height / 2.0,
        "50% grandchild",
    );
}

#[test]
fn column_grow_reflows_percentage_descendants() {
    let (layout, style) = percentage_descendants(
        "flex-direction: column; height: 200px;",
        "flex-grow: 1; height: auto;",
        false,
    );
    assert_eq!(layout.dimensions.content.height, 200.0);
    assert_descendant_heights(&layout, 200.0, 200.0);
    assert!(matches!(
        style.get("height"),
        Some(ComputedValue::Keyword(value)) if value == "auto"
    ));
}

#[test]
fn column_grow_preserves_content_box_for_percentage_descendants() {
    for box_sizing in ["content-box", "border-box"] {
        let (layout, _) = percentage_descendants(
            "flex-direction: column; height: 200px;",
            &format!(
                "flex-grow: 1; padding: 10px; border: 5px solid; margin: 5px; \
                 box-sizing: {box_sizing};"
            ),
            false,
        );
        assert_descendant_heights(&layout, 160.0, 160.0);
        assert_eq!(layout.children[0].total_height(), 200.0);
        assert_eq!(layout.children[0].children[0].dimensions.content.y, 20.0);
    }
}

#[test]
fn column_grow_reflows_nested_flex_and_grid_containers() {
    for display in [
        "display: flex; flex-direction: column;",
        "display: grid; grid-template-rows: 100%;",
    ] {
        let (layout, _) = percentage_descendants(
            "flex-direction: column; height: 200px;",
            &format!("flex-grow: 1; {display}"),
            false,
        );
        assert_descendant_heights(&layout, 200.0, 200.0);
    }
}

#[test]
fn column_min_height_requires_a_definite_flex_basis_for_percentages() {
    for (basis, expected_child_height) in [("auto", 0.0), ("0px", 200.0)] {
        let (layout, _) = percentage_descendants(
            "flex-direction: column; min-height: 200px;",
            &format!("flex-grow: 1; flex-basis: {basis};"),
            false,
        );
        assert_eq!(layout.dimensions.content.height, 200.0);
        assert_descendant_heights(&layout, 200.0, expected_child_height);
    }
}

#[test]
fn row_stretch_reflows_percentage_descendants() {
    let (layout, _) = percentage_descendants("height: 200px;", "", false);
    assert_descendant_heights(&layout, 200.0, 200.0);
}

#[test]
fn row_non_stretched_auto_height_keeps_percentages_indefinite() {
    let (layout, _) = percentage_descendants("height: 200px; align-items: flex-start;", "", false);
    assert_descendant_heights(&layout, 0.0, 0.0);
}

#[test]
fn row_stretch_preserves_explicit_height_and_align_self() {
    let (explicit, _) = percentage_descendants("height: 200px;", "height: 40px;", false);
    assert_descendant_heights(&explicit, 40.0, 40.0);
    let (aligned, _) = percentage_descendants("height: 200px;", "align-self: flex-start;", false);
    assert_descendant_heights(&aligned, 0.0, 0.0);
}

#[test]
fn row_stretch_uses_the_natural_line_height() {
    let (layout, _) = percentage_descendants("", "", true);
    assert_eq!(layout.dimensions.content.height, 120.0);
    assert_descendant_heights(&layout, 120.0, 120.0);
}

#[test]
fn flex_reflow_respects_item_max_height() {
    for direction in ["row", "column"] {
        let (layout, _) = percentage_descendants(
            &format!("flex-direction: {direction}; height: 200px;"),
            "flex-grow: 1; max-height: 120px;",
            false,
        );
        assert_descendant_heights(&layout, 120.0, 120.0);
    }
}

#[test]
fn column_grow_uses_the_containers_content_height() {
    let (layout, _) = percentage_descendants(
        "flex-direction: column; height: 200px; box-sizing: border-box; \
         padding: 10px; border: 5px solid;",
        "flex-grow: 1;",
        false,
    );
    assert_eq!(layout.dimensions.content.height, 170.0);
    assert_descendant_heights(&layout, 170.0, 170.0);
}

#[test]
fn column_grow_redistributes_space_after_an_item_reaches_max_height() {
    let container = NodeHandle::element("main");
    for _ in 0..2 {
        let item = NodeHandle::element("article");
        let child = NodeHandle::element("section");
        child.append_child(NodeHandle::element("div"));
        item.append_child(child);
        container.append_child(item);
    }
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "main { display: flex; flex-direction: column; height: 210px; gap: 10px; } \
             article { flex-grow: 1; } article:first-child { max-height: 50px; } \
             section { height: 100%; } div { height: 50%; }",
        )
        .unwrap(),
    );
    let layout = layout_tree(
        &container,
        &mut resolver,
        Rect {
            width: 300.0,
            ..Rect::default()
        },
    )
    .unwrap();
    assert_descendant_heights(&layout, 50.0, 50.0);
    let second = &layout.children[1];
    assert_eq!(second.dimensions.content.y, 60.0);
    assert_eq!(second.dimensions.content.height, 150.0);
    assert_eq!(second.children[0].dimensions.content.height, 150.0);
    assert_eq!(
        second.children[0].children[0].dimensions.content.height,
        75.0
    );
    assert_eq!(layout.dimensions.content.height, 210.0);
}

#[test]
fn column_percentage_max_height_uses_the_flex_container_height() {
    let container = NodeHandle::element("main");
    for _ in 0..2 {
        let item = NodeHandle::element("article");
        let child = NodeHandle::element("section");
        child.append_child(NodeHandle::element("div"));
        item.append_child(child);
        container.append_child(item);
    }
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "main { display: flex; flex-direction: column; height: 210px; gap: 10px; } \
             article { flex-grow: 1; } article:first-child { max-height: 50%; } \
             section { height: 100%; } div { height: 50%; }",
        )
        .unwrap(),
    );
    let layout = layout_tree(
        &container,
        &mut resolver,
        Rect {
            width: 300.0,
            ..Rect::default()
        },
    )
    .unwrap();
    assert_descendant_heights(&layout, 100.0, 100.0);
    assert_eq!(layout.children[1].dimensions.content.y, 110.0);
    assert_eq!(layout.children[1].dimensions.content.height, 100.0);
}

#[test]
fn nested_stretched_rows_do_not_repeat_both_layout_passes_at_every_depth() {
    let root = NodeHandle::element("div");
    let mut parent = root.clone();
    for _ in 0..32 {
        let child = NodeHandle::element("div");
        parent.append_child(child.clone());
        parent = child;
    }
    parent.append_child(NodeHandle::element("aside"));
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "div { display: flex; width: 20px; min-width: 0; } \
             aside { height: 20px; width: 20px; }",
        )
        .unwrap(),
    );
    let layout = layout_tree(
        &root,
        &mut resolver,
        Rect {
            width: 20.0,
            ..Rect::default()
        },
    )
    .unwrap();
    let mut current = &layout;
    for _ in 0..33 {
        assert_eq!(current.dimensions.content.height, 20.0);
        current = &current.children[0];
    }
    assert_eq!(current.dimensions.content.height, 20.0);
}
