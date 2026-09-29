use omoikane::{
    css::{Origin, StyleResolver, parse_stylesheet},
    dom::NodeHandle,
    html::TreeBuilder,
    js::JsRuntime,
    layout::{LayoutBox, Rect, layout_tree},
};

fn runtime(style: &str) -> JsRuntime {
    let html = format!("<style>{style}</style><div id='target'></div>");
    JsRuntime::with_document(TreeBuilder::parse(&html).document())
        .expect("create JavaScript runtime")
}

fn check(runtime: &mut JsRuntime, script: &str) {
    assert!(
        runtime
            .eval(script)
            .unwrap_or_else(|error| panic!("{script}: {error}"))
            .to_boolean(),
        "{script}"
    );
}

#[test]
fn logical_sizing_inset_border_and_radius_follow_vertical_rtl_flow() {
    let mut runtime = runtime(
        "#target { position:absolute; writing-mode:vertical-rl; direction:rtl; \
         inline-size:20px; block-size:30px; inset-inline-start:7px; \
         border-block-start:2px solid red; border-start-start-radius:4px; }",
    );
    check(
        &mut runtime,
        "(() => { const s = getComputedStyle(document.getElementById('target')); \
         return s.width === '30px' && s.height === '20px' && s.bottom === '7px' && \
         s.borderRightWidth === '2px' && s.borderBottomRightRadius === '4px'; })()",
    );
}

#[test]
fn physical_and_logical_properties_share_cascade_order() {
    let mut runtime = runtime(
        "#target { width:11px; block-size:22px; writing-mode:vertical-rl; \
         border-right-style:solid; border-right-width:1px; \
         border-block-start-width:5px; }",
    );
    check(
        &mut runtime,
        "(() => { const s = getComputedStyle(document.getElementById('target')); \
         return s.width === '22px' && s.borderRightWidth === '5px'; })()",
    );
}

#[test]
fn logical_mapping_uses_importance_and_rtl_direction() {
    let mut runtime = runtime(
        "#target { direction:rtl; margin-left:3px; margin-inline-start:7px; \
         margin-left:11px !important; padding-inline-start:5px; \
         border-inline-start-style:solid; border-inline-start-width:thin; }",
    );
    check(
        &mut runtime,
        "(() => { const s = getComputedStyle(document.getElementById('target')); \
         return s.marginLeft === '11px' && s.marginRight === '7px' && \
         s.paddingRight === '5px' && s.borderRightWidth === '1px'; })()",
    );
}

#[test]
fn cssom_exposes_logical_shorthands() {
    let mut runtime = runtime("");
    check(
        &mut runtime,
        "(() => { const s = document.getElementById('target').style; \
         s.inset = '1px 2px'; s.setProperty('inset-inline', '3px 4px'); \
         s.borderInline = '2px solid red'; \
         return s.inset === '1px 2px' && s.insetInline === '3px 4px' && \
         s.borderInline === '2px solid red' && \
         s.getPropertyValue('border-inline-start-width') === '2px'; })()",
    );
}

#[test]
fn logical_border_cssom_serializes_omitted_components_and_calc() {
    let mut runtime = runtime("");
    check(
        &mut runtime,
        "(() => { const s = document.getElementById('target').style; \
         s.borderInline = 'double'; if (s.borderInline !== 'double') return false; \
         s.borderInlineStart = 'green'; if (s.borderInlineStart !== 'green') return false; \
         s.borderInlineEndWidth = '0'; if (s.borderInlineEndWidth !== '0px') return false; \
         s.borderInlineEnd = 'calc(10px - 0.5em) dotted red'; \
         return s.borderInlineEnd === 'calc(-0.5em + 10px) dotted red'; })()",
    );
}

#[test]
fn logical_insets_and_dimensions_position_vertical_boxes() {
    let document = NodeHandle::document();
    let box_node = NodeHandle::element("div");
    document.append_child(box_node.clone());
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "div { position:absolute; writing-mode:vertical-rl; \
             inset-inline-start:10px; inset-block-start:20px; \
             inline-size:40px; block-size:30px; margin:0; padding:0; border:0 }",
        )
        .unwrap(),
    );
    let layout = layout_tree(
        &document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 300.0,
            height: 200.0,
        },
    )
    .unwrap();
    fn find_box<'a>(layout: &'a LayoutBox, node: &NodeHandle) -> Option<&'a LayoutBox> {
        if layout.node == *node {
            Some(layout)
        } else {
            layout
                .children
                .iter()
                .find_map(|child| find_box(child, node))
        }
    }
    let rect = find_box(&layout, &box_node)
        .expect("box appears in layout")
        .dimensions
        .content;
    assert_eq!(
        (rect.x, rect.y, rect.width, rect.height),
        (250.0, 10.0, 30.0, 40.0)
    );
}

#[test]
fn positioned_auto_width_respects_logical_min_and_max_sizes() {
    let mut runtime = runtime(
        "#target { position:absolute; left:10px; top:10px; \
         min-inline-size:54px; max-inline-size:54px; height:20px; }",
    );
    check(
        &mut runtime,
        "document.getElementById('target').getBoundingClientRect().width === 54",
    );
}

#[test]
fn later_physical_sides_override_logical_sides_in_vertical_layout() {
    let document = NodeHandle::document();
    let box_node = NodeHandle::element("div");
    document.append_child(box_node.clone());
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "div { writing-mode:vertical-rl; width:20px; height:20px; \
             padding-block-start:5px; padding-right:8px; \
             margin-block-start:4px; margin-right:9px; \
             border-right-style:solid; border-block-start-width:2px; \
             border-right-width:3px; }",
        )
        .unwrap(),
    );
    let layout = layout_tree(
        &document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 300.0,
            height: 200.0,
        },
    )
    .unwrap();
    fn find_box<'a>(layout: &'a LayoutBox, node: &NodeHandle) -> Option<&'a LayoutBox> {
        if layout.node == *node {
            Some(layout)
        } else {
            layout
                .children
                .iter()
                .find_map(|child| find_box(child, node))
        }
    }
    let dimensions = find_box(&layout, &box_node)
        .expect("box appears in layout")
        .dimensions;
    assert_eq!(dimensions.padding.right, 8.0);
    assert_eq!(dimensions.margin.right, 9.0);
    assert_eq!(dimensions.border.right, 3.0);
}

#[test]
fn later_physical_auto_offset_clears_logical_offset() {
    let mut runtime = runtime(
        "#target { position:absolute; writing-mode:vertical-rl; width:20px; height:20px; \
         inset-block-start:20px; right:auto; top:0; }",
    );
    check(
        &mut runtime,
        "getComputedStyle(document.getElementById('target')).right === 'auto'",
    );
}
