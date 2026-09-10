use super::*;
use crate::css::parse_stylesheet;

thread_local! {
    static REFERENCE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static REFERENCE_NODE_COPIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(super) fn use_reference() -> bool {
    REFERENCE.with(std::cell::Cell::get)
}

fn reference<T>(paint: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            REFERENCE.with(|flag| flag.set(false));
        }
    }
    REFERENCE.with(|flag| flag.set(true));
    let _reset = Reset;
    paint()
}

fn compare_layout(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    viewport: Rect,
) -> (Canvas, usize) {
    let geometry = format!("{layout:?}");
    REFERENCE_NODE_COPIES.with(|count| count.set(0));
    let expected = reference(|| paint_layout_with_fonts(layout, resolver, viewport, Vec::new()));
    let reference_copies = REFERENCE_NODE_COPIES.with(std::cell::Cell::get);
    assert!(reference_copies > 0);
    REFERENCE_NODE_COPIES.with(|count| count.set(0));
    let actual = paint_layout_with_fonts(layout, resolver, viewport, Vec::new());
    assert_eq!(REFERENCE_NODE_COPIES.with(std::cell::Cell::get), 0);
    assert_pixels(&actual, &expected, "immutable-layout");
    assert_eq!(
        format!("{layout:?}"),
        geometry,
        "painting must leave layout unchanged"
    );
    (actual, reference_copies)
}

fn assert_pixels(actual: &Canvas, expected: &Canvas, case: &str) {
    assert_eq!(
        (actual.width(), actual.height()),
        (expected.width(), expected.height())
    );
    if actual.pixels() != expected.pixels() {
        let first = actual
            .pixels()
            .chunks_exact(4)
            .zip(expected.pixels().chunks_exact(4))
            .position(|(a, b)| a != b)
            .unwrap();
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/output");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("anonymized-transform-tiles.offset.actual.png"),
            actual.encode_png(),
        )
        .unwrap();
        std::fs::write(
            directory.join("anonymized-transform-tiles.offset.expected.png"),
            expected.encode_png(),
        )
        .unwrap();
        panic!(
            "tile pixels differ for {case} at ({}, {}); PNGs saved in tests/output",
            first % actual.width() as usize,
            first / actual.width() as usize
        );
    }
}

#[test]
fn tiles_share_immutable_layout_and_keep_the_source_surface_bound() {
    for (tiles, children) in [(1, 32), (4, 32), (16, 32), (4, 128)] {
        let width = 2048 * tiles;
        let node = NodeHandle::element("div");
        node.set_attribute("class", "host");
        for i in 0..children {
            let child = NodeHandle::element("i");
            child.set_attribute("style", format!("left:{}px", (i * 29) % width));
            node.append_child(child);
        }
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(Origin::Author, parse_stylesheet(&format!(
            ".host {{display:block;position:relative;width:{width}px;height:4px;background:red;transform-origin:0 0;transform:scaleX({});}} i {{display:block;position:absolute;top:0;width:1px;height:1px;background:blue;}}",256.0 / width as f32
        )).unwrap());
        let viewport = Rect {
            x: 0.0,
            y: 0.0,
            width: 256.0,
            height: 4.0,
        };
        let layout = crate::layout::layout_tree(&node, &mut resolver, viewport).unwrap();
        TRANSFORM_SURFACE_STATS.with(|stats| stats.set((0, 0, 0)));
        let (canvas, reference_copies) = compare_layout(&layout, &mut resolver, viewport);
        assert_eq!(reference_copies, tiles * (children + 1));
        assert_eq!(canvas.pixel(128, 2), Some(Color::rgb(255, 0, 0)));
        let (count, pixels, largest) = TRANSFORM_SURFACE_STATS.with(std::cell::Cell::get);
        assert_eq!(count, tiles);
        assert_eq!(pixels, (width * 4) as u64);
        assert_eq!(largest, 2048 * 4);
        eprintln!(
            "tiles={tiles} nodes={} reference_subtree_copies={reference_copies} largest_surface_pixels={largest}",
            children + 1
        );
    }
}

#[test]
fn tiled_offsets_preserve_nested_effects_clipping_text_and_stacking() {
    use base64::Engine as _;
    let mut image = Canvas::new(2, 2);
    image.set_pixel(0, 0, Color::rgb(255, 0, 0));
    image.set_pixel(1, 0, Color::rgb(0, 255, 0));
    image.set_pixel(0, 1, Color::rgb(0, 0, 255));
    image.set_pixel(1, 1, Color::rgb(255, 255, 255));
    let image_url = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(image.encode_png())
    );
    for effect in [
        "",
        "opacity:0.5",
        "filter:blur(1px)",
        "clip-path:circle(48%)",
        "overflow:hidden",
        "border-radius:4px;border:2px solid green",
    ] {
        let document = crate::html::TreeBuilder::parse(&format!(r#"
            <style>
            html,body {{ margin:0; }}
            main {{ position:relative;width:8200px;height:32px;transform-origin:0 0;transform:scaleX(0.5);background:#eee; }}
            section {{ position:absolute;left:4090px;top:3px;width:60px;height:22px;transform-origin:0 0;transform:translateX(5px) rotate(5deg);{effect};background:red; }}
            span {{ background:blue;color:white;font:10px sans-serif; }}
            b {{ position:absolute;left:30px;top:-1px;width:6px;height:24px;background:yellow;z-index:-1; }}
            i {{ position:absolute;left:50px;top:5px;width:20px;height:8px;background:purple;z-index:2; }}
            img {{ position:absolute;left:5px;top:12px;width:10px;height:6px;object-fit:fill;z-index:3; }}
            li {{ margin-left:8px;font-size:8px;list-style-type:square; }}
            </style><main><section><b></b><span>tile text</span><li>marker</li><img src="{image_url}"><i></i></section></main>
        "#)).document();
        let viewport = Rect {
            x: 0.0,
            y: 0.0,
            width: 4200.0,
            height: 40.0,
        };
        let expected = reference(|| render_document(&document, viewport).unwrap());
        let actual = render_document(&document, viewport).unwrap();
        assert_pixels(&actual, &expected, effect);
        assert!(actual.pixels().chunks_exact(4).any(|pixel| pixel[3] > 0));
    }
}

// Frozen pre-#649 tile implementation: deliberately copies and translates the
// subtree to provide an independent pixel reference for the shared-layout path.
pub(super) fn paint_reference(
    canvas: &mut Canvas,
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    inherited_clip: Option<Rect>,
    viewport: Rect,
    text_fonts: &[Arc<Font>],
    web_fonts: Option<&WebFontRegistry>,
    offset: PaintOffset,
) {
    let mut shifted;
    let layout = if offset.x != 0.0 || offset.y != 0.0 {
        shifted = layout.clone();
        translate_layout_for_paint(&mut shifted, offset.x, offset.y);
        &shifted
    } else {
        layout
    };
    let Some(inverse) = layout.transform.inverse() else {
        return;
    };
    let canvas_bounds = Rect {
        x: 0.0,
        y: 0.0,
        width: canvas.width() as f32,
        height: canvas.height() as f32,
    };
    let destination_bounds = if let Some(clip) = inherited_clip {
        let Some(bounds) = intersect(clip, canvas_bounds) else {
            return;
        };
        bounds
    } else {
        canvas_bounds
    };
    let required_source = transformed_rect_bounds(destination_bounds, inverse);
    let required_source = Rect {
        x: required_source.x - 1.0,
        y: required_source.y - 1.0,
        width: required_source.width + 2.0,
        height: required_source.height + 2.0,
    };
    let Some(source_region) = intersect(subtree_paint_bounds(layout, resolver), required_source)
    else {
        return;
    };
    let source_x0 = source_region.x.floor() as i32;
    let source_y0 = source_region.y.floor() as i32;
    let source_x1 = (source_region.x + source_region.width).ceil() as i32;
    let source_y1 = (source_region.y + source_region.height).ceil() as i32;
    let tile_size = TRANSFORM_SURFACE_TILE_SIZE as i32;

    // Paint source-space tiles instead of a viewport-sized surface. This keeps
    // pixels that begin outside the viewport but transform into view, while the
    // inverse-mapped destination bounds and tiling prevent unbounded allocation.
    for tile_y in (source_y0..source_y1).step_by(tile_size as usize) {
        for tile_x in (source_x0..source_x1).step_by(tile_size as usize) {
            let tile_x1 = (tile_x + tile_size).min(source_x1);
            let tile_y1 = (tile_y + tile_size).min(source_y1);
            let tile_width = (tile_x1 - tile_x).max(1) as u32;
            let tile_height = (tile_y1 - tile_y).max(1) as u32;
            let mut translated_layout = layout.clone();
            translate_layout_for_paint(&mut translated_layout, -(tile_x as f32), -(tile_y as f32));
            let translated_viewport = Rect {
                x: viewport.x - tile_x as f32,
                y: viewport.y - tile_y as f32,
                width: viewport.width,
                height: viewport.height,
            };
            let mut offscreen = Canvas::new(tile_width, tile_height);
            // A transformed element establishes a paint containment boundary
            // for positioned descendants. Its ancestor clip is applied later
            // in destination space; local overflow/clip-path is painted here.
            paint_box_internal_untransformed(
                &mut offscreen,
                &translated_layout,
                resolver,
                None,
                translated_viewport,
                true,
                text_fonts,
                web_fonts,
                PaintOffset::default(),
            );
            let tile_transform = layout
                .transform
                .multiply(AffineTransform::translate(tile_x as f32, tile_y as f32));
            composite_affine(
                canvas,
                &offscreen,
                tile_transform,
                inherited_clip,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    width: tile_width as f32,
                    height: tile_height as f32,
                },
            );
        }
    }
}

fn translate_layout_for_paint(layout: &mut LayoutBox, dx: f32, dy: f32) {
    REFERENCE_NODE_COPIES.with(|count| count.set(count.get() + 1));
    layout.dimensions.content.x += dx;
    layout.dimensions.content.y += dy;
    for line in &mut layout.lines {
        line.rect.x += dx;
        line.rect.y += dy;
        for fragment in &mut line.fragments {
            fragment.rect.x += dx;
            fragment.rect.y += dy;
        }
    }
    if let Some(marker) = &mut layout.marker {
        marker.x += dx;
        marker.y += dy;
    }
    layout.transform = AffineTransform::translate(dx, dy)
        .multiply(layout.transform)
        .multiply(AffineTransform::translate(-dx, -dy));
    for child in &mut layout.children {
        translate_layout_for_paint(child, dx, dy);
    }
}
