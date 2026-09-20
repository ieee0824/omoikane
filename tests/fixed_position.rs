//! Fixed descendants keep the coordinates of their own containing block.
use omoikane::{
    css::{Origin, StyleResolver, parse_stylesheet},
    dom::NodeHandle,
    layout::{LayoutBox, Rect, layout_tree},
};

fn box_for<'a>(layout: &'a LayoutBox, node: &NodeHandle) -> &'a LayoutBox {
    fn find<'a>(layout: &'a LayoutBox, node: &NodeHandle) -> Option<&'a LayoutBox> {
        if layout.node == *node {
            Some(layout)
        } else {
            layout.children.iter().find_map(|child| find(child, node))
        }
    }
    find(layout, node).expect("node must have a layout box")
}

#[test]
fn nested_fixed_boxes_keep_viewport_coordinates() {
    let document = NodeHandle::document();
    let outer = NodeHandle::element("div");
    let fixed = NodeHandle::element("div");
    let nested = NodeHandle::element("div");
    let absolute = NodeHandle::element("div");
    for (node, id) in [
        (&outer, "outer"),
        (&fixed, "fixed"),
        (&nested, "nested"),
        (&absolute, "absolute"),
    ] {
        node.set_attribute("id", id);
    }
    document.append_child(outer.clone());
    outer.append_child(fixed.clone());
    fixed.append_child(nested.clone());
    nested.append_child(absolute.clone());
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "div { margin:0; border:0; padding:0; width:40px; height:20px } \
         #outer { position:absolute; left:20px; top:20px; width:280px } \
         #fixed { position:fixed; left:20px; top:130px; width:280px } \
         #nested { position:fixed; left:20px; top:60px } \
         #absolute { position:absolute; left:3px; top:4px; width:10px; height:10px }",
        )
        .unwrap(),
    );
    let layout = layout_tree(
        &document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 320.0,
            height: 180.0,
        },
    )
    .unwrap();
    for (node, expected) in [
        (&outer, (20.0, 20.0)),
        (&fixed, (20.0, 130.0)),
        (&nested, (20.0, 60.0)),
        (&absolute, (23.0, 64.0)),
    ] {
        let rect = box_for(&layout, node).dimensions.content;
        assert_eq!(
            (rect.x, rect.y),
            expected,
            "{}",
            node.get_attribute("id").unwrap()
        );
    }
}

#[test]
fn fixed_uses_the_transformed_ancestors_padding_box_through_positioned_wrappers() {
    for trigger in [
        "transform:translate(0px)",
        "perspective:400px",
        "contain:layout",
        "contain:paint",
    ] {
        let document = NodeHandle::document();
        let outer = NodeHandle::element("div");
        let wrapper = NodeHandle::element("div");
        let fixed = NodeHandle::element("div");
        let nested = NodeHandle::element("div");
        for (node, id) in [
            (&outer, "outer"),
            (&wrapper, "wrapper"),
            (&fixed, "fixed"),
            (&nested, "nested"),
        ] {
            node.set_attribute("id", id);
        }
        document.append_child(outer.clone());
        outer.append_child(wrapper.clone());
        wrapper.append_child(fixed.clone());
        fixed.append_child(nested.clone());
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(Origin::Author, parse_stylesheet(&format!(
            "div {{ margin:0; border:0; padding:0 }} \
             #outer {{ position:absolute; left:30px; top:40px; width:200px; height:100px; padding:10px; border:5px solid black; {trigger} }} \
             #wrapper {{ position:relative; left:70px; top:30px; width:100px; height:50px }} \
             #fixed {{ position:fixed; left:10%; top:20%; width:10%; height:25% }} \
             #nested {{ position:fixed; right:10px; top:5px; width:20px; height:10px }}"
        )).unwrap());
        let layout = layout_tree(
            &document,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 320.0,
                height: 180.0,
            },
        )
        .unwrap();
        assert_eq!(
            box_for(&layout, &fixed).dimensions.content,
            Rect {
                x: 57.0,
                y: 69.0,
                width: 22.0,
                height: 30.0
            },
            "{trigger}"
        );
        assert_eq!(
            box_for(&layout, &nested).dimensions.content,
            Rect {
                x: 225.0,
                y: 50.0,
                width: 20.0,
                height: 10.0
            },
            "{trigger}"
        );
    }
}

#[test]
fn fixed_bottom_uses_an_auto_height_containing_block_through_a_wrapper() {
    let document = NodeHandle::document();
    let outer = NodeHandle::element("div");
    let wrapper = NodeHandle::element("div");
    let spacer = NodeHandle::element("div");
    let fixed = NodeHandle::element("div");
    for (node, id) in [
        (&outer, "outer"),
        (&wrapper, "wrapper"),
        (&spacer, "spacer"),
        (&fixed, "fixed"),
    ] {
        node.set_attribute("id", id);
    }
    document.append_child(outer.clone());
    outer.append_child(wrapper.clone());
    wrapper.append_child(spacer);
    wrapper.append_child(fixed.clone());
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "div { margin:0; border:0; padding:0 } \
         #outer { position:absolute; left:30px; top:40px; width:200px; transform:translate(0px) } \
         #wrapper { position:relative; left:70px; top:30px } \
         #spacer { height:80px } \
         #fixed { position:fixed; right:10px; bottom:10px; width:20px; height:10px }",
        )
        .unwrap(),
    );
    let layout = layout_tree(
        &document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 320.0,
            height: 180.0,
        },
    )
    .unwrap();
    assert_eq!(box_for(&layout, &outer).dimensions.content.height, 80.0);
    assert_eq!(
        box_for(&layout, &fixed).dimensions.content,
        Rect {
            x: 200.0,
            y: 100.0,
            width: 20.0,
            height: 10.0
        }
    );
}

#[test]
fn fixed_auto_axes_follow_static_position_but_explicit_logical_insets_do_not() {
    for (insets, expected) in [
        ("top:10px", (40.0, 10.0)),
        ("left:10px", (10.0, 30.0)),
        (
            "direction:rtl;inset-inline-start:20px;inset-block-start:30px",
            (280.0, 30.0),
        ),
    ] {
        let document = NodeHandle::document();
        let outer = NodeHandle::element("div");
        let fixed = NodeHandle::element("div");
        outer.set_attribute("id", "outer");
        fixed.set_attribute("id", "fixed");
        document.append_child(outer.clone());
        outer.append_child(fixed.clone());
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(Origin::Author,parse_stylesheet(&format!(
            "div {{ margin:0; border:0; padding:0 }} #outer {{ position:absolute;left:40px;top:30px;width:200px;height:100px }} #fixed {{ position:fixed;width:20px;height:10px;{insets} }}"
        )).unwrap());
        let layout = layout_tree(
            &document,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 320.0,
                height: 180.0,
            },
        )
        .unwrap();
        let rect = box_for(&layout, &fixed).dimensions.content;
        assert_eq!((rect.x, rect.y), expected, "{insets}");
    }
}

#[test]
fn fixed_coordinates_survive_formatting_context_placement() {
    for mode in ["flex", "grid", "table", "float", "inline-block", "vertical"] {
        for transformed in [false, true] {
            let document = NodeHandle::document();
            let outer = NodeHandle::element(if mode == "table" { "table" } else { "div" });
            let item = NodeHandle::element(if mode == "table" { "td" } else { "div" });
            let fixed = NodeHandle::element("div");
            for (node, id) in [(&outer, "outer"), (&item, "item"), (&fixed, "fixed")] {
                node.set_attribute("id", id);
            }
            document.append_child(outer.clone());
            if mode == "table" {
                let row = NodeHandle::element("tr");
                outer.append_child(row.clone());
                row.append_child(item.clone());
            } else {
                outer.append_child(item.clone());
            }
            item.append_child(fixed.clone());
            let formatting = match mode {
                "flex" => "#outer {display:flex;justify-content:center;align-items:center}",
                "grid" => "#outer {display:grid;grid-template-columns:80px;justify-content:center}",
                "table" => "#outer {border-spacing:0} #item {vertical-align:middle}",
                "float" => "#item {float:right}",
                "inline-block" => "#outer {text-align:right} #item {display:inline-block}",
                "vertical" => "#outer {writing-mode:vertical-rl}",
                _ => unreachable!(),
            };
            let transform = if transformed {
                "transform:translate(0px)"
            } else {
                ""
            };
            let mut resolver = StyleResolver::new();
            resolver.add_stylesheet(Origin::Author, parse_stylesheet(&format!(
                "* {{margin:0;border:0;padding:0}} #outer {{position:absolute;left:40px;top:50px;width:200px;height:100px;{transform}}} \
                 #item {{width:80px;height:40px}} #fixed {{position:fixed;left:10px;top:20px;width:20px;height:10px}} {formatting}"
            )).unwrap());
            let layout = layout_tree(
                &document,
                &mut resolver,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 320.0,
                    height: 180.0,
                },
            )
            .unwrap();
            let rect = box_for(&layout, &fixed).dimensions.content;
            assert_eq!(
                (rect.x, rect.y),
                if transformed {
                    (50.0, 70.0)
                } else {
                    (10.0, 20.0)
                },
                "{mode}, transformed={transformed}"
            );
        }
    }
}

#[test]
fn original_fixed_fixture_paints_at_viewport_coordinates_in_all_validity_states() {
    use omoikane::{cdp::CdpSession, frame::render_browser_frame};
    use serde_json::json;
    let html = include_str!("fixtures/anonymized-nested-fixed/states.html");
    let url = format!(
        "data:text/html,{}",
        html.bytes()
            .map(|b| format!("%{b:02X}"))
            .collect::<String>()
    );
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch("Page.navigate", json!({"url":url}))
        .unwrap();
    let red = [220, 60, 40, 255];
    let green = [40, 160, 80, 255];
    let gray = [150, 150, 150, 255];
    for (phase, script, colors) in [
        ("initial", "void 0", [red, green, red, red]),
        (
            "changed",
            "document.getElementById('native').value='filled';document.getElementById('custom').internals.setValidity({customError:true},'invalid')",
            [red, red, green, red],
        ),
        (
            "disabled",
            "document.getElementById('group').disabled=true",
            [green, gray, gray, green],
        ),
    ] {
        let result = session
            .dispatch("Runtime.evaluate", json!({"expression":script}))
            .unwrap();
        assert!(result.get("exceptionDetails").is_none(), "{result}");
        let frame = render_browser_frame(&mut session, 320, 180, 16).unwrap();
        if let Some(root) = std::env::var_os("OMOIKANE_FIXED_IMAGES") {
            let root = std::path::PathBuf::from(root);
            std::fs::create_dir_all(&root).unwrap();
            let file = std::fs::File::create(
                root.join(format!("anonymized-nested-fixed.{phase}.actual.png")),
            )
            .unwrap();
            let mut encoder = png::Encoder::new(file, 320, 180);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(frame.pixels())
                .unwrap();
        }
        for ((x, y), color) in [(280, 30), (80, 90), (240, 90), (280, 140)]
            .into_iter()
            .zip(colors)
        {
            let offset = (y * 320 + x) * 4;
            assert_eq!(
                &frame.pixels()[offset..offset + 4],
                color,
                "{phase}: {x},{y}"
            );
        }
    }
}
