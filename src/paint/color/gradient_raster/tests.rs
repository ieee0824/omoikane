use super::*;

fn backdrop(width: u32, height: u32) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    for (i, pixel) in canvas.pixels.chunks_exact_mut(4).enumerate() {
        pixel.copy_from_slice(&[
            (i * 17) as u8,
            (i * 31) as u8,
            (i * 47) as u8,
            (i * 13) as u8,
        ]);
    }
    canvas
}

// Keep scalar evaluation for every pixel as the reference. Geometry and stop
// resolution are shared, while the new constant-row decision is disabled.
fn scalar(canvas: &mut Canvas, gradient: &Gradient, area: Rect, clip: Option<Rect>) {
    let Some(area) = normalize_rect(area) else {
        return;
    };
    let Some(draw) = clip
        .and_then(normalize_rect)
        .map(|clip| intersect(area, clip))
        .unwrap_or(Some(area))
    else {
        return;
    };
    paint::<false>(canvas, gradient, area, draw);
}

#[test]
fn gradient_row_reuse_matches_scalar_pixels_with_alpha_clipping_and_stops() {
    let gradients = [
        "linear-gradient(red, blue)",
        "linear-gradient(0deg, red, blue)",
        "linear-gradient(179.99998deg, red, blue)",
        "linear-gradient(360deg, red, blue)",
        "linear-gradient(45deg, red, blue)",
        "linear-gradient(90deg, red 0%, red 25%, blue 25%, blue 75%, red 75%, red 100%)",
        "linear-gradient(rgba(255,0,0,0.3), 35%, rgba(0,0,255,0.8))",
        "repeating-linear-gradient(180deg, red 0px, blue 3px, red 5px)",
        "radial-gradient(red, blue)",
        "conic-gradient(from 45deg, red, blue)",
    ];
    let areas = [
        Rect {
            x: 0.0,
            y: 0.0,
            width: 83.0,
            height: 61.0,
        },
        Rect {
            x: -1.25,
            y: -2048.25,
            width: 500.0,
            height: 8000.0,
        },
        Rect {
            x: 0.5,
            y: -0.5,
            width: 83.0,
            height: 4.0,
        },
    ];
    let clips = [
        None,
        Some(Rect {
            x: 2.2,
            y: 3.8,
            width: 44.7,
            height: 37.5,
        }),
        Some(Rect {
            x: 120.0,
            y: 0.0,
            width: 5.0,
            height: 8.0,
        }),
    ];
    for css in gradients {
        let gradient = parse_gradient(css).unwrap_or_else(|| panic!("fixture: {css}"));
        for area in areas {
            for clip in clips {
                let mut expected = backdrop(83, 61);
                let mut actual = expected.clone();
                scalar(&mut expected, &gradient, area, clip);
                paint_gradient(&mut actual, &gradient, area, clip);
                assert_eq!(actual.pixels, expected.pixels, "{css}, {area:?}, {clip:?}");
            }
        }
    }
}

#[test]
fn tall_vertical_gradient_reuses_each_row_without_changing_pixels() {
    let gradient = parse_gradient("linear-gradient(red, blue)").unwrap();
    let area = Rect {
        x: 0.0,
        y: 0.0,
        width: 1280.0,
        height: 8000.0,
    };
    let mut expected = backdrop(1280, 680);
    let mut actual = expected.clone();
    scalar(&mut expected, &gradient, area, None);
    REUSED_ROWS.with(|count| count.set(0));
    paint_gradient(&mut actual, &gradient, area, None);
    assert_eq!(actual.pixels, expected.pixels);
    assert_eq!(REUSED_ROWS.with(std::cell::Cell::get), 680);
}

#[test]
fn equal_endpoint_colors_do_not_hide_interior_stops_or_radial_variation() {
    let gradient = parse_gradient(
        "linear-gradient(90deg, red 0%, red 25%, blue 25%, blue 75%, red 75%, red 100%)",
    )
    .unwrap();
    let area = Rect {
        x: 0.0,
        y: 0.0,
        width: 64.0,
        height: 8.0,
    };
    let mut canvas = Canvas::new(64, 8);
    REUSED_ROWS.with(|count| count.set(0));
    paint_gradient(&mut canvas, &gradient, area, None);
    assert_eq!(canvas.pixel(1, 4), Some(Color::rgb(255, 0, 0)));
    assert_eq!(canvas.pixel(32, 4), Some(Color::rgb(0, 0, 255)));
    assert_eq!(canvas.pixel(62, 4), Some(Color::rgb(255, 0, 0)));
    assert_eq!(REUSED_ROWS.with(std::cell::Cell::get), 0);
    for css in ["radial-gradient(red, blue)", "conic-gradient(red, blue)"] {
        let gradient = parse_gradient(css).unwrap();
        let geometry = Geometry::resolve(&gradient, area);
        assert!(
            geometry
                .constant_row_value(&gradient.kind, 0, 64, 4)
                .is_none()
        );
    }
}

#[test]
fn constant_row_requires_finite_parameters_and_a_nonempty_span() {
    let mut gradient = parse_gradient("linear-gradient(red, blue)").unwrap();
    let area = Rect {
        x: 0.0,
        y: 0.0,
        width: 64.0,
        height: 8000.0,
    };
    let geometry = Geometry::resolve(&gradient, area);
    assert!(
        geometry
            .constant_row_value(&gradient.kind, 5, 5, 4)
            .is_none()
    );
    gradient.kind = GradientKind::Linear {
        angle_deg: f32::NAN,
    };
    let geometry = Geometry::resolve(&gradient, area);
    assert!(
        geometry
            .constant_row_value(&gradient.kind, 0, 64, 4)
            .is_none()
    );
}
