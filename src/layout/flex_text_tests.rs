use super::*;
use crate::css::{Origin, parse_stylesheet};
use crate::html::TreeBuilder;

fn render_with(html: &str, css: &str, mutate: impl FnOnce(&NodeHandle)) -> LayoutBox {
    let document = TreeBuilder::parse(html).document();
    mutate(&document);
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());
    let mut fonts = crate::font::WebFontRegistry::new();
    fonts.push(
        "Liberation Sans",
        crate::font::FontWeight(400),
        crate::font::FontStyle::Normal,
        crate::font::Font::load_from_bytes(
            include_bytes!("../../tests/fixtures/acid2/LiberationSans-Regular.ttf").to_vec(),
        )
        .unwrap(),
    );
    crate::layout::with_layout_fonts(Vec::new(), Some(std::sync::Arc::new(fonts)), || {
        crate::layout::layout_tree(
            &document,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 800.0,
                height: 600.0,
            },
        )
        .unwrap()
    })
}

fn by_id<'a>(layout: &'a LayoutBox, id: &str) -> &'a LayoutBox {
    fn find<'a>(layout: &'a LayoutBox, id: &str) -> Option<&'a LayoutBox> {
        if layout
            .node
            .attributes()
            .is_some_and(|attrs| attrs.get("id").is_some_and(|v| v == id))
        {
            return Some(layout);
        }
        layout.children.iter().find_map(|child| find(child, id))
    }
    find(layout, id).unwrap_or_else(|| panic!("missing #{id}"))
}

fn render(html: &str, css: &str) -> LayoutBox {
    render_with(html, css, |_| {})
}

const CSS: &str = "html,body{margin:0;padding:0}body{padding:24px;font-family:'Liberation Sans';font-size:16px;line-height:24px}.row{display:flex;align-items:center;gap:12px;width:400px;height:60px;margin-bottom:16px}.icon{width:36px;height:36px;flex-shrink:0}";

fn painted_text(layout: &LayoutBox) -> String {
    layout
        .lines
        .iter()
        .flat_map(|line| &line.fragments)
        .filter_map(|fragment| match &fragment.content {
            InlineFragmentContent::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn direct_flex_text_matches_a_span_without_changing_the_dom() {
    let root = render(
        "<div id='bare' class='row'><div class='icon'></div>Visible text</div><div id='wrapped' class='row'><div class='icon'></div><span id='label'>Visible text</span></div>",
        CSS,
    );
    let bare = by_id(&root, "bare");
    let wrapped = by_id(&root, "wrapped");
    assert_eq!(bare.children.len(), 2);
    assert_eq!(bare.node.child_nodes().len(), 2);
    let text = &bare.children[1];
    let span = by_id(&root, "label");
    assert_eq!(text.node.node_type(), NodeType::Text);
    assert_eq!(text.node.parent_node().unwrap(), bare.node);
    assert_eq!(painted_text(text), "Visible text");
    assert_eq!(text.dimensions.content.x, span.dimensions.content.x);
    assert_eq!(
        text.dimensions.content.y - bare.dimensions.content.y,
        span.dimensions.content.y - wrapped.dimensions.content.y
    );
    assert!((text.dimensions.content.width - span.dimensions.content.width).abs() < 0.001);
    assert_eq!(text.dimensions.content.height, 24.0);
}

#[test]
fn adjacent_text_nodes_and_comments_form_one_anonymous_item() {
    let root = render_with("<div id='bare' class='row'></div>", CSS, |document| {
        let row = document.query_selector("#bare").unwrap();
        row.append_child(NodeHandle::text("Vis"));
        row.append_child(NodeHandle::text("ible"));
        row.append_child(NodeHandle::comment("ignored"));
        row.append_child(NodeHandle::text(" text"));
    });
    let row = by_id(&root, "bare");
    assert_eq!(row.children.len(), 1);
    assert_eq!(row.node.child_nodes().len(), 4);
    assert_eq!(painted_text(&row.children[0]), "Visible text");
}

#[test]
fn whitespace_only_runs_do_not_add_flex_items_but_nbsp_does() {
    let root = render(
        "<div id='bare' class='row'> \n<div class='icon'></div> \t<div class='icon'></div> </div><div id='nbsp' class='row'>&nbsp;</div>",
        CSS,
    );
    let row = by_id(&root, "bare");
    assert_eq!(row.children.len(), 2);
    assert_eq!(
        row.children[1].dimensions.content.x - row.children[0].dimensions.content.x,
        48.0
    );
    let nbsp = by_id(&root, "nbsp");
    assert_eq!(nbsp.children.len(), 1);
    assert!(nbsp.children[0].dimensions.content.width > 0.0);
}

#[test]
fn mixed_flex_text_wraps_and_flows_in_rows_and_columns() {
    for mode in [
        "flex-direction:row",
        "flex-direction:row;flex-wrap:wrap",
        "flex-direction:column",
    ] {
        let root = render(
            "<div id='bare' class='row'><div class='icon'></div>Visible text wraps around</div><div id='wrapped' class='row'><div class='icon'></div><span id='label'>Visible text wraps around</span></div>",
            &format!("{CSS}.row{{width:140px;height:auto;align-items:stretch;{mode}}}"),
        );
        let bare = by_id(&root, "bare");
        let wrapped = by_id(&root, "wrapped");
        assert_eq!(bare.children.len(), 2, "{mode}");
        let text = &bare.children[1];
        let span = by_id(&root, "label");
        assert_eq!(text.lines.len(), span.lines.len(), "{mode}");
        assert!(text.lines.len() >= 2, "{mode}");
        assert_eq!(
            text.dimensions.content.height, span.dimensions.content.height,
            "{mode}"
        );
        assert!(
            (text.dimensions.content.width - span.dimensions.content.width).abs() < 0.001,
            "{mode}"
        );
        assert_eq!(
            text.dimensions.content.y - bare.dimensions.content.y,
            span.dimensions.content.y - wrapped.dimensions.content.y,
            "{mode}"
        );
        assert_eq!(
            painted_text(text)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
            "Visible text wraps around"
        );
    }
}

#[test]
fn column_center_shrink_wraps_both_anonymous_text_and_span() {
    let root = render(
        "<div id='bare' class='row'><div class='icon'></div>Visible text</div><div id='wrapped' class='row'><div class='icon'></div><span id='label'>Visible text</span></div>",
        &format!("{CSS}.row{{width:140px;height:auto;flex-direction:column;align-items:center}}"),
    );
    let bare = by_id(&root, "bare");
    let text = &bare.children[1];
    let span = by_id(&root, "label");
    assert!(span.dimensions.content.width < 100.0);
    assert!((span.dimensions.content.width - text.dimensions.content.width).abs() < 0.001);
    assert!((span.dimensions.content.x - text.dimensions.content.x).abs() < 0.001);
    assert!(
        (text.dimensions.content.x - (24.0 + (140.0 - text.dimensions.content.width) / 2.0)).abs()
            < 0.001
    );
}

#[test]
fn nested_auto_width_flex_includes_text_and_gap_in_its_intrinsic_width() {
    let root = render(
        "<div class='row'><div id='bare' class='brand'><div class='icon'></div>Benchmark Portal</div></div><div class='row'><div id='wrapped' class='brand'><div class='icon'></div><span>Benchmark Portal</span></div></div>",
        &format!("{CSS}.brand{{display:flex;align-items:center;gap:12px}}"),
    );
    let bare = by_id(&root, "bare");
    let wrapped = by_id(&root, "wrapped");
    let text = &bare.children[1];
    assert_eq!(painted_text(text), "Benchmark Portal");
    assert_eq!(text.lines.len(), 1);
    assert!(bare.dimensions.content.width > 150.0);
    assert!((bare.dimensions.content.width - wrapped.dimensions.content.width).abs() < 0.001);
    assert!(
        (text.dimensions.content.x + text.dimensions.content.width
            - bare.dimensions.content.x
            - bare.dimensions.content.width)
            .abs()
            < 0.001
    );
}
