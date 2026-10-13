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

#[test]
fn logical_box_cssom_expands_normalizes_and_removes_shorthands() {
    let mut runtime = runtime("");
    check(
        &mut runtime,
        "(() => { const s = document.getElementById('target').style;
         for (const prefix of ['margin', 'padding']) {
           for (const axis of ['block', 'inline']) {
             const name = prefix + '-' + axis;
             s.setProperty(name, '0 0');
             if (s.getPropertyValue(name) !== '0px' ||
                 s.getPropertyValue(name + '-start') !== '0px' ||
                 s.getPropertyValue(name + '-end') !== '0px') return false;
             s.setProperty(name + '-end', '12px');
             if (s.getPropertyValue(name) !== '0px 12px') return false;
             s.removeProperty(name);
             if (s.getPropertyValue(name + '-start') !== '' ||
                 s.getPropertyValue(name + '-end') !== '') return false;
           }
         }
         s.marginBlock = 'auto auto';
         return s.marginBlock === 'auto' && s.marginBlockEnd === 'auto'; })()",
    );
}

#[test]
fn logical_box_invalid_values_preserve_existing_declarations() {
    let mut runtime = runtime("");
    check(
        &mut runtime,
        "(() => { const s = document.getElementById('target').style;
         for (const name of ['padding-block', 'padding-inline']) {
           s.setProperty(name, '2px 3px');
           for (const value of ['auto', '-1px', '4px -1%', '1px, 2px', '1px 2px 3px', 'calc(1)']) {
             if (CSS.supports(name, value)) return false;
             s.setProperty(name, value);
             if (s.getPropertyValue(name) !== '2px 3px') return false;
           }
         }
         s.marginInlineStart = '7px';
         for (const value of ['none', '1px 2px', '10', '1px, 2px', 'calc(1)']) {
           if (CSS.supports('margin-inline-start', value)) return false;
           s.marginInlineStart = value;
           if (s.marginInlineStart !== '7px') return false;
         }
         return true; })()",
    );
}

#[test]
fn logical_box_resolved_values_follow_live_flow_and_containing_width() {
    let mut runtime = runtime(
        "#target { width:100px; margin-inline:10% calc(10% + 2px); \
         padding-block:calc(1px - 2px) 5%; }",
    );
    check(
        &mut runtime,
        "(() => { const target = document.getElementById('target');
         const parent = document.createElement('div');
         parent.style.width = '200px'; document.body.appendChild(parent);
         parent.appendChild(target);
         const s = getComputedStyle(target);
         if (s.marginInline !== '20px 22px' || s.marginLeft !== '20px' ||
             s.paddingBlock !== '0px 10px' || s.paddingTop !== '0px') return false;
         target.style.direction = 'rtl';
         if (s.marginInlineStart !== '20px' || s.marginRight !== '20px') return false;
         target.style.writingMode = 'vertical-rl';
         return s.marginInlineStart === s.marginBottom &&
             s.paddingBlockStart === s.paddingRight && s.paddingBlockEnd === s.paddingLeft;
         })()",
    );
}

#[test]
fn logical_box_css_text_preserves_priority_and_css_wide_values() {
    let mut runtime = runtime("");
    check(
        &mut runtime,
        "(() => { const s = document.getElementById('target').style;
         s.cssText = 'margin-inline: 2px 3px !important; padding-block: 0 0';
         if (s.marginInlineStart !== '2px' || s.marginInlineEnd !== '3px' ||
             s.paddingBlock !== '0px' || s.getPropertyPriority('margin-inline') !== 'important') return false;
         s.setProperty('margin-inline-end', '4px');
         if (s.marginInline !== '' || s.getPropertyPriority('margin-inline') !== '') return false;
         s.paddingBlock = 'inherit';
         if (s.paddingBlock !== 'inherit' || s.paddingBlockStart !== 'inherit') return false;
         s.paddingBlockEnd = 'initial';
         if (s.paddingBlock !== '') return false;
         s.marginBlock = '5px 6px';
         return s.removeProperty('margin-block') === '5px 6px' && s.marginBlockStart === ''; })()",
    );
}

#[test]
fn will_change_transform_anchors_absolute_and_fixed_percentage_edges() {
    for position in ["absolute", "fixed"] {
        for hint in [
            "transform",
            "opacity, TRANSFORM",
            "translate",
            "rotate",
            "scale",
            "perspective",
        ] {
            let mut runtime = JsRuntime::with_document(
                TreeBuilder::parse(&format!(
                    "<style>#container {{ width:200px; will-change:{hint}; }} \
                     #parent {{ width:0; }} #target {{ position:{position}; width:20px; \
                     margin-block-end:10%; padding-inline-end:calc(10% + 40px); }}</style>\
                     <div id='container'><div id='parent'><div id='target'></div></div></div>"
                ))
                .document(),
            )
            .expect("create runtime");
            check(
                &mut runtime,
                "(() => { const s = getComputedStyle(document.getElementById('target'));
                 return s.marginBlockEnd === '20px' && s.paddingInlineEnd === '60px'; })()",
            );
        }
    }
}

#[test]
fn logical_box_pending_substitution_keeps_specified_shorthand() {
    let mut runtime = runtime("");
    check(
        &mut runtime,
        "(() => { const target = document.getElementById('target'); const s = target.style;
         s.setProperty('--edges', '3px 4px');
         s.marginBlock = 'var(--edges)';
         if (s.marginBlock !== 'var(--edges)' || s.marginBlockStart !== '') return false;
         const c = getComputedStyle(target);
         if (c.marginBlock !== '3px 4px') return false;
         s.marginBlockEnd = '8px';
         if (s.marginBlock !== '' || c.marginBlock !== '3px 8px') return false;
         s.setProperty('margin-block', 'var(--edges)', 'important');
         return s.getPropertyPriority('margin-block') === 'important'; })()",
    );
}

#[test]
fn invalid_logical_box_shorthands_do_not_partially_enter_stylesheet_cascade() {
    let mut runtime = runtime(
        "#target { padding-block:2px 3px; padding-block:4px -1px; \
         margin-inline:5px 6px; margin-inline:7px none; \
         padding-inline:8px 9px; padding-inline:inherit 10px; }",
    );
    check(
        &mut runtime,
        "(() => { const s = getComputedStyle(document.getElementById('target'));
         return s.paddingBlock === '2px 3px' && s.marginInline === '5px 6px' &&
             s.paddingInline === '8px 9px' && !CSS.supports('padding-inline', 'INHERIT 10px'); })()",
    );
}

#[test]
fn logical_box_math_keeps_element_dependent_lengths_until_cascade() {
    let mut runtime = runtime("");
    check(
        &mut runtime,
        "(() => { const target = document.getElementById('target');
         const parent = document.createElement('div'); document.body.appendChild(parent);
         parent.appendChild(target); parent.appendChild(document.createElement('div'));
         if (!CSS.supports('margin-inline', 'calc(sibling-index() * 2px)') ||
             !CSS.supports('padding-block', 'calc(sibling-count() * 3px)')) return false;
         target.style.marginInline = 'calc(sibling-index() * 2px)';
         target.style.paddingBlock = 'calc(sibling-count() * 3px)';
         const s = getComputedStyle(target);
         if (s.marginInline !== '2px' || s.paddingBlock !== '6px') return false;
         parent.prepend(document.createElement('div'));
         return s.marginInline === '4px' && s.paddingBlock === '9px'; })()",
    );
}
