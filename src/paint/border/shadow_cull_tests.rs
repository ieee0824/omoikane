use super::*;

// Frozen pre-cull behavior for both mask-allocating branches. The nonrounded
// zero-blur rectangle-band branch is unchanged and remains its own reference.
fn paint_reference(
    canvas: &mut Canvas,
    border: Rect,
    shadow: &BoxShadow,
    clip: Option<Rect>,
    radii: Option<(f32, f32, f32, f32)>,
) {
    if shadow.blur_radius <= 0.0 && radii.is_none() {
        paint_outer_box_shadow(canvas, border, shadow, clip, radii);
        return;
    }
    let spread = shadow.spread_radius;
    let rect = Rect {
        x: border.x + shadow.offset_x - spread,
        y: border.y + shadow.offset_y - spread,
        width: border.width + spread * 2.0,
        height: border.height + spread * 2.0,
    };
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return;
    }
    let blur = shadow.blur_radius;
    let sigma = blur * 0.5;
    let direct = sigma > 0.5 && sigma <= 2.0;
    let passes = gaussian_box_blur_radii(blur);
    let margin = if blur <= 0.0 {
        0
    } else if direct {
        (sigma * 3.0).ceil() as u32
    } else {
        passes.iter().copied().fold(0u32, u32::saturating_add)
    }
    .saturating_add(u32::from(blur > 0.0));
    let margin_f = margin as f32;
    let x = (rect.x - margin_f).floor() as i32;
    let y = (rect.y - margin_f).floor() as i32;
    let width = (rect.width + margin_f * 2.0).ceil() as u32 + 2;
    let height = (rect.height + margin_f * 2.0).ceil() as u32 + 2;
    if width == 0 || height == 0 {
        return;
    }
    let mut mask = Canvas::new(width, height);
    let local = Rect {
        x: rect.x - x as f32,
        y: rect.y - y as f32,
        width: rect.width,
        height: rect.height,
    };
    let color = shadow.color;
    if let Some((tl, tr, br, bl)) = radii {
        mask.fill_rounded_rect(
            local,
            Color::rgba(color.r, color.g, color.b, 255),
            tl + spread,
            tr + spread,
            br + spread,
            bl + spread,
            None,
        );
    } else {
        mask.fill_rect(local, Color::rgba(color.r, color.g, color.b, 255));
    }
    if blur > 0.0 {
        if direct {
            mask.gaussian_blur_alpha_zero_padded(sigma);
        } else {
            mask.box_blur_alpha_radii_zero_padded(&passes);
        }
    }
    let box_local = Rect {
        x: border.x - x as f32,
        y: border.y - y as f32,
        width: border.width,
        height: border.height,
    };
    knock_out_shadow_interior(&mut mask, box_local, radii);
    canvas.composite_canvas_clipped(
        &mask,
        x,
        y,
        color.r,
        color.g,
        color.b,
        color.a as f32 / 255.0,
        clip,
    );
}

fn shadow(blur: f32, spread: f32, offset_x: f32, offset_y: f32, alpha: u8) -> BoxShadow {
    BoxShadow {
        offset_x,
        offset_y,
        blur_radius: blur,
        spread_radius: spread,
        color: Color::rgba(12, 45, 91, alpha),
        inset: false,
    }
}

#[test]
fn outer_shadow_cull_corpus_matches_precull_rgba_exactly() {
    let origins = [
        (-200.0, 8.0),
        (200.0, 8.0),
        (8.0, -200.0),
        (8.0, 200.0),
        (-20.0, 8.0),
        (64.0, 8.0),
        (8.0, -14.0),
        (8.0, 48.0),
        (-0.25, 7.5),
        (57.75, 43.5),
    ];
    let clips = [
        None,
        Some(Rect {
            x: 3.25,
            y: 2.75,
            width: 41.5,
            height: 30.25,
        }),
        Some(Rect {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
        }),
    ];
    for &(x, y) in &origins {
        for blur in [0.0, 1.0, 1.01, 4.0, 4.01, 6.0] {
            for radii in [
                None,
                Some((0.0, 0.0, 0.0, 0.0)),
                Some((2.0, 5.5, 9.0, 1.25)),
            ] {
                for clip in clips {
                    let border = Rect {
                        x,
                        y,
                        width: 20.0,
                        height: 14.0,
                    };
                    let mut actual = Canvas::new(64, 48);
                    let mut expected = Canvas::new(64, 48);
                    if blur > 1.0 {
                        let backing = Rect {
                            x: 0.0,
                            y: 0.0,
                            width: 64.0,
                            height: 48.0,
                        };
                        actual.fill_rect(backing, Color::rgba(90, 120, 180, 128));
                        expected.fill_rect(backing, Color::rgba(90, 120, 180, 128));
                    }
                    for sh in [
                        shadow(blur, 1.25, -3.5, 2.75, 128),
                        shadow(blur, -1.5, 2.25, -4.5, 255),
                    ] {
                        paint_outer_box_shadow(&mut actual, border, &sh, clip, radii);
                        paint_reference(&mut expected, border, &sh, clip, radii);
                    }
                    assert_eq!(
                        actual.pixels(),
                        expected.pixels(),
                        "origin=({x},{y}) blur={blur} radii={radii:?} clip={clip:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn offcanvas_border_shadow_tail_and_negative_offset_remain_visible() {
    for (x, offset) in [(-20.0, 0.0), (80.0, -70.0)] {
        let mut canvas = Canvas::new(64, 48);
        let border = Rect {
            x,
            y: 10.0,
            width: 20.0,
            height: 14.0,
        };
        paint_outer_box_shadow(
            &mut canvas,
            border,
            &shadow(6.0, 0.0, offset, 0.0, 255),
            None,
            Some((3.0, 3.0, 3.0, 3.0)),
        );
        assert!(
            canvas.pixels().chunks_exact(4).any(|pixel| pixel[3] > 0),
            "offscreen box at {x} still casts visible shadow"
        );
    }
}

#[test]
#[ignore = "deterministic profiling fixture; no timing threshold"]
fn profile_offcanvas_outer_shadow_fixed_fixture() {
    let started = std::time::Instant::now();
    let mut checksum = 0u64;
    for frame in 0..20 {
        let mut canvas = Canvas::new(128, 96);
        for index in 0..32 {
            let border = Rect {
                x: 12.25,
                y: 200.0 + index as f32 * 100.0,
                width: 128.0,
                height: 72.0,
            };
            paint_outer_box_shadow(
                &mut canvas,
                border,
                &shadow(6.0, 0.0, 0.0, 0.0, 128),
                None,
                Some((4.0, 4.0, 4.0, 4.0)),
            );
        }
        assert!(canvas.pixels().iter().all(|&byte| byte == 0));
        checksum = checksum.wrapping_add(canvas.pixels().len() as u64 + frame);
        std::hint::black_box(&canvas);
    }
    eprintln!(
        "OFFCANVAS_SHADOW_PROFILE frames=20 shadows_per_frame=32 elapsed_ms={} checksum={checksum}",
        started.elapsed().as_secs_f64() * 1000.0
    );
}

fn render_translated_shadow(declarations: &str) -> Canvas {
    let document =
        crate::html::TreeBuilder::parse("<html><body><div id='target'></div></body></html>")
            .document();
    let mut resolver = crate::css::StyleResolver::new();
    resolver.add_stylesheet(crate::css::Origin::Author,crate::css::parse_stylesheet(&format!(
        "html,body{{margin:0;background:white}}#target{{position:absolute;top:8px;width:20px;height:14px;background:white;border-radius:3px;box-shadow:0 0 6px rgba(0,0,0,.5);{declarations}}}"
    )).unwrap());
    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 64.0,
        height: 48.0,
    };
    let layout = crate::layout::layout_tree(
        &document.query_selector("html").unwrap(),
        &mut resolver,
        viewport,
    )
    .unwrap();
    super::super::paint_layout_with_fonts(&layout, &mut resolver, viewport, Vec::new())
}

#[test]
fn transformed_offscreen_source_shadow_matches_visible_integer_translation() {
    for opacity in ["", "opacity:.75;"] {
        let actual =
            render_translated_shadow(&format!("left:-80px;transform:translateX(80px);{opacity}"));
        let expected = render_translated_shadow(&format!("left:0px;{opacity}"));
        assert_eq!(
            actual.pixels(),
            expected.pixels(),
            "source outside viewport must survive local tile culling: {opacity}"
        );
        assert!(
            actual
                .pixels()
                .chunks_exact(4)
                .any(|p| p[0] < 255 && p[3] > 0),
            "visible shadow must not be blank"
        );
    }
}

#[test]
fn shadow_mask_bounds_cull_respects_halfopen_edges_and_wide_arithmetic() {
    for (x, y, w, h, expected) in [
        (-1, 0, 1, 1, false),
        (-1, 0, 2, 1, true),
        (63, 0, 1, 1, true),
        (64, 0, 1, 1, false),
        (0, -1, 1, 1, false),
        (0, -1, 1, 2, true),
        (0, 47, 1, 1, true),
        (0, 48, 1, 1, false),
        (0, 0, 0, 1, false),
        (0, 0, 1, 0, false),
        (i32::MIN, 0, 1, 1, false),
        (i32::MAX, 0, 2, 1, false),
        (i32::MIN, 0, u32::MAX, 1, true),
        (0, i32::MIN, 1, u32::MAX, true),
    ] {
        assert_eq!(
            shadow_buffer_intersects_canvas(x, y, w, h, 64, 48),
            expected,
            "({x},{y},{w},{h})"
        );
    }
    assert!(!shadow_buffer_intersects_canvas(0, 0, 1, 1, 0, 48));
    assert!(!shadow_buffer_intersects_canvas(0, 0, 1, 1, 64, 0));
    // Preserve the old renderer's pathological i32 dimension semantics rather
    // than broadening this optimization to unrepresentable destination sizes.
    assert!(shadow_buffer_intersects_canvas(0, 0, 1, 1, u32::MAX, 48));
}
