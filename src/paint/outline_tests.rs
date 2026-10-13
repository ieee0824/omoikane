use crate::css::StyleResolver;
use crate::html::TreeBuilder;
use crate::layout::{LayoutBox, Rect, layout_tree};

use super::{BorderRegion, Canvas, Color, render_document, rounded_rect_pixel_coverage};

fn white_page(html: &str, width: f32, height: f32) -> Canvas {
    let document = TreeBuilder::parse(html).document();
    render_document(
        &document,
        Rect {
            width,
            height,
            ..Rect::default()
        },
    )
    .unwrap()
}

#[test]
fn solid_outline_fixture_paints_width_color_offset_and_rounded_edges() {
    let pixels = white_page(
        include_str!("../../tests/fixtures/anonymized-outline-paint/solid.html"),
        260.0,
        100.0,
    );
    let white = Some(Color::rgb(255, 255, 255));
    let gray = Some(Color::rgb(221, 221, 221));

    assert_eq!(pixels.pixel(18, 30), Some(Color::rgb(255, 0, 0)));
    assert_eq!(pixels.pixel(17, 17), Some(Color::rgb(255, 0, 0)));
    assert_eq!(pixels.pixel(16, 30), white);
    assert_eq!(pixels.pixel(20, 30), gray);

    assert_eq!(pixels.pixel(54, 30), Some(Color::rgb(0, 0, 255)));
    assert_eq!(pixels.pixel(58, 30), white, "positive offset leaves a gap");
    assert_eq!(pixels.pixel(60, 30), gray);

    assert_eq!(pixels.pixel(103, 30), Some(Color::rgb(0, 128, 0)));
    assert_eq!(pixels.pixel(101, 30), gray, "negative offset is inset");
    assert_eq!(pixels.pixel(105, 30), gray);

    assert_eq!(
        pixels.pixel(138, 30),
        Some(Color::rgb(128, 0, 128)),
        "the box's own overflow must not clip its outline"
    );
    assert_eq!(pixels.pixel(136, 30), white);

    assert_eq!(pixels.pixel(190, 18), Some(Color::rgb(255, 165, 0)));
    assert_eq!(pixels.pixel(178, 18), white, "rounded corner stays clear");
    assert_eq!(pixels.pixel(218, 30), white, "none paints no outline");
    assert_eq!(pixels.pixel(18, 70), Some(Color::rgb(0, 128, 0)));
    assert_eq!(pixels.pixel(58, 70), white, "zero width paints nothing");
}

#[test]
fn outline_does_not_change_following_boxes_layout() {
    let document = TreeBuilder::parse(
        "<html><body style='margin:0'><div id='first' style='width:20px;height:20px;outline:10px solid red'></div><div id='second' style='width:20px;height:20px;background:blue'></div></body></html>",
    )
    .document();
    let viewport = Rect {
        width: 60.0,
        height: 60.0,
        ..Rect::default()
    };
    let mut resolver = StyleResolver::new();
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    fn box_with_id<'a>(layout: &'a LayoutBox, id: &str) -> Option<&'a LayoutBox> {
        if layout.node.get_attribute("id").as_deref() == Some(id) {
            return Some(layout);
        }
        layout
            .children
            .iter()
            .find_map(|child| box_with_id(child, id))
    }
    let first = box_with_id(&layout, "first").unwrap();
    let second = box_with_id(&layout, "second").unwrap();
    assert_eq!(
        second.dimensions.content.y - first.dimensions.content.y,
        20.0
    );
}

#[test]
fn ancestor_overflow_clips_outline_but_large_negative_offset_still_paints() {
    let pixels = white_page(
        "<html style='background:white'><body style='margin:0'>\
         <div style='position:absolute;left:20px;top:20px;width:30px;height:30px;overflow:hidden'>\
           <div style='position:absolute;left:20px;top:5px;width:10px;height:10px;outline:3px solid red'></div>\
         </div>\
         <div style='position:absolute;left:80px;top:20px;width:10px;height:10px;outline:2px solid blue;outline-offset:-100px'></div>\
         </body></html>",
        120.0,
        60.0,
    );
    assert_eq!(pixels.pixel(38, 30), Some(Color::rgb(255, 0, 0)));
    assert_eq!(pixels.pixel(51, 30), Some(Color::rgb(255, 255, 255)));
    assert_eq!(pixels.pixel(84, 24), Some(Color::rgb(0, 0, 255)));
}

#[test]
fn zero_sized_box_still_paints_a_complete_outline() {
    let pixels = white_page(
        "<html style='background:white'><body style='margin:0'>\
         <div style='position:absolute;left:20px;top:20px;width:0;height:0;outline:3px solid red'></div>\
         </body></html>",
        60.0,
        60.0,
    );
    assert_eq!(pixels.pixel(17, 17), Some(Color::rgb(255, 0, 0)));
    assert_eq!(pixels.pixel(22, 22), Some(Color::rgb(255, 0, 0)));
    assert_eq!(pixels.pixel(16, 20), Some(Color::rgb(255, 255, 255)));
}

#[test]
fn zero_sized_outline_inherits_currentcolor_keyword_from_parent() {
    // WPT css/css-ui/outline-019.html: the inherited currentColor keyword
    // resolves against the child's own color, rather than the parent's red.
    let pixels = white_page(
        "<html style='background:white'><body style='margin:0'>\
         <div style='position:absolute;left:20px;top:20px;color:red;outline-color:currentColor'>\
           <div style='width:0;height:0;color:green;outline-color:inherit;outline-style:solid;outline-width:3px'></div>\
         </div></body></html>",
        60.0,
        60.0,
    );
    for (x, y) in [(17, 17), (22, 17), (17, 22), (22, 22), (20, 20)] {
        assert_eq!(
            pixels.pixel(x, y),
            Some(Color::rgb(0, 128, 0)),
            "inherited currentColor at ({x}, {y})"
        );
    }
    assert_eq!(pixels.pixel(16, 20), Some(Color::rgb(255, 255, 255)));
}

#[test]
fn large_negative_offset_clamps_each_axis_independently() {
    let pixels = white_page(
        "<html style='background:white'><body style='margin:0'>\
         <div style='position:absolute;left:20px;top:20px;width:2px;height:100px;outline:2px solid blue;outline-offset:-20px'></div>\
         </body></html>",
        60.0,
        140.0,
    );
    let white = Some(Color::rgb(255, 255, 255));
    let blue = Some(Color::rgb(0, 0, 255));
    assert_eq!(pixels.pixel(20, 37), white);
    assert_eq!(pixels.pixel(20, 38), blue);
    assert_eq!(pixels.pixel(19, 50), blue);
    assert_eq!(pixels.pixel(22, 50), blue);
    assert_eq!(pixels.pixel(20, 101), blue);
    assert_eq!(pixels.pixel(20, 102), white);
}

#[test]
fn opacity_and_transform_surfaces_keep_all_outer_outline_edges() {
    for (effects, border_left) in [
        ("opacity:.5", 20),
        ("transform:translateX(30px)", 50),
        ("opacity:.5;transform:translateX(30px)", 50),
    ] {
        let html = format!(
            "<html style='background:white'><body style='margin:0'>\
             <div style='position:absolute;left:20px;top:20px;width:20px;height:20px;outline:4px solid red;{effects}'></div>\
             </body></html>"
        );
        let pixels = white_page(&html, 100.0, 70.0);
        for (x, y) in [
            (border_left - 3, 30),
            (border_left + 22, 30),
            (border_left + 10, 17),
            (border_left + 10, 42),
        ] {
            let edge = pixels.pixel(x, y).unwrap();
            assert_eq!(edge.r, 255, "{effects} ({x}, {y})");
            assert!(
                edge.g < 255 && edge.b < 255,
                "{effects} clipped outline at ({x}, {y}): {edge:?}"
            );
        }
        assert_eq!(
            pixels.pixel(border_left - 5, 30),
            Some(Color::rgb(255, 255, 255)),
            "{effects}"
        );
    }
}

#[test]
fn rounded_outline_scans_each_translucent_pixel_once() {
    let outer = Rect {
        x: 5.25,
        y: 4.5,
        width: 84.5,
        height: 27.5,
    };
    let inner = Rect {
        x: 7.25,
        y: 6.5,
        width: 80.5,
        height: 23.5,
    };
    let outer_radii = (12.0, 10.0, 9.0, 11.0);
    let inner_radii = (10.0, 8.0, 7.0, 9.0);
    let mut canvas = Canvas::new(100, 40);
    canvas.fill_rounded_rect_annulus(
        outer,
        outer_radii.0,
        outer_radii.1,
        outer_radii.2,
        outer_radii.3,
        inner,
        inner_radii.0,
        inner_radii.1,
        inner_radii.2,
        inner_radii.3,
        Color {
            r: 255,
            g: 0,
            b: 0,
            a: 128,
        },
        None,
        BorderRegion::All,
        2.0,
    );
    for y in 0..canvas.height() {
        for x in 0..canvas.width() {
            let coverage = (rounded_rect_pixel_coverage(x as i32, y as i32, outer, outer_radii)
                - rounded_rect_pixel_coverage(x as i32, y as i32, inner, inner_radii))
            .clamp(0.0, 1.0);
            let expected_alpha = (128.0 * coverage).round() as u8;
            assert_eq!(canvas.pixel(x, y).unwrap().a, expected_alpha, "({x}, {y})");
        }
    }
}
