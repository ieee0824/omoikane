use std::hint::black_box;
use std::time::{Duration, Instant};

use super::{Canvas, Color, Image, blend_pixel, intersect, normalize_rect};
use crate::layout::Rect;

struct BaselineCase {
    name: &'static str,
    image: Image,
    destination: Rect,
    clip: Option<Rect>,
    opacity: f32,
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn patterned_image(width: u32, height: u32) -> Image {
    const ALPHAS: [u8; 8] = [0, 1, 63, 127, 128, 191, 254, 255];
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            let index = (y * width + x) as usize;
            pixels.extend_from_slice(&[
                ((x * 53 + y * 17 + 3) % 256) as u8,
                ((x * 11 + y * 71 + 29) % 256) as u8,
                ((x * 97 + y * 7 + 101) % 256) as u8,
                ALPHAS[index % ALPHAS.len()],
            ]);
        }
    }
    Image::new(width, height, pixels).unwrap()
}

fn premultiplied_alpha_image() -> Image {
    Image::new(
        4,
        2,
        vec![
            255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 240, 20, 30, 1, 10, 220, 40, 63, 30, 50,
            230, 191, 200, 180, 10, 254, 5, 15, 25, 127,
        ],
    )
    .unwrap()
}

fn patterned_canvas(width: u32, height: u32) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    for y in 0..height {
        for x in 0..width {
            canvas.set_pixel(
                x,
                y,
                Color::rgba(
                    ((x * 19 + y * 3 + 7) % 256) as u8,
                    ((x * 5 + y * 23 + 31) % 256) as u8,
                    ((x * 13 + y * 29 + 47) % 256) as u8,
                    ((x * 37 + y * 41 + 89) % 256) as u8,
                ),
            );
        }
    }
    canvas
}

// Keep this independent from the production sampler. It freezes the operation types,
// ordering, and accumulation order that the axis-plan optimization must preserve.
fn reference_box_filter_sample(
    image: &Image,
    source_left: f32,
    source_top: f32,
    source_right: f32,
    source_bottom: f32,
) -> Color {
    let x0 = source_left.floor().max(0.0) as u32;
    let y0 = source_top.floor().max(0.0) as u32;
    let x1 = source_right.ceil().min(image.width as f32) as u32;
    let y1 = source_bottom.ceil().min(image.height as f32) as u32;
    let mut total_weight = 0.0f64;
    let mut alpha_sum = 0.0f64;
    let mut red_sum = 0.0f64;
    let mut green_sum = 0.0f64;
    let mut blue_sum = 0.0f64;

    for source_y in y0..y1 {
        let y_weight = (source_bottom.min(source_y as f32 + 1.0) - source_top.max(source_y as f32))
            .max(0.0) as f64;
        for source_x in x0..x1 {
            let x_weight = (source_right.min(source_x as f32 + 1.0)
                - source_left.max(source_x as f32))
            .max(0.0) as f64;
            let weight = x_weight * y_weight;
            if weight == 0.0 {
                continue;
            }
            let index = ((source_y * image.width + source_x) * 4) as usize;
            let alpha = image.pixels[index + 3] as f64;
            total_weight += weight;
            alpha_sum += alpha * weight;
            red_sum += image.pixels[index] as f64 * alpha * weight;
            green_sum += image.pixels[index + 1] as f64 * alpha * weight;
            blue_sum += image.pixels[index + 2] as f64 * alpha * weight;
        }
    }

    if total_weight == 0.0 || alpha_sum == 0.0 {
        return Color::rgba(0, 0, 0, 0);
    }
    let channel = |sum: f64| (sum / alpha_sum).round().clamp(0.0, 255.0) as u8;
    Color::rgba(
        channel(red_sum),
        channel(green_sum),
        channel(blue_sum),
        (alpha_sum / total_weight).round().clamp(0.0, 255.0) as u8,
    )
}

pub(super) fn reference_draw_image_scaled_clipped_with_opacity(
    canvas: &mut Canvas,
    image: &Image,
    destination: Rect,
    clip: Option<Rect>,
    opacity: f32,
) {
    let opacity = if opacity.is_finite() {
        opacity.clamp(0.0, 1.0)
    } else {
        1.0
    };
    if opacity <= 0.0 {
        return;
    }
    let Some(destination) = normalize_rect(destination) else {
        return;
    };
    if destination.width <= 0.0 || destination.height <= 0.0 {
        return;
    };
    let mut area = destination;

    if let Some(clip_rect) = clip.and_then(normalize_rect) {
        let Some(intersection) = intersect(area, clip_rect) else {
            return;
        };
        area = intersection;
    }

    let x0 = area.x.floor().max(0.0) as i32;
    let y0 = area.y.floor().max(0.0) as i32;
    let x1 = (area.x + area.width).ceil().min(canvas.width as f32) as i32;
    let y1 = (area.y + area.height).ceil().min(canvas.height as f32) as i32;
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    canvas.record_animation_regions_from_scaled_image(image, destination, area);
    let image = image.sampled_image(crate::layout::image_animation_time_ms());
    let shrink_x = destination.width < image.width as f32;
    let shrink_y = destination.height < image.height as f32;
    let source_per_dest_x = image.width as f32 / destination.width;
    let source_per_dest_y = image.height as f32 / destination.height;

    for dest_y in y0..y1 {
        for dest_x in x0..x1 {
            let u = (dest_x as f32 - destination.x) / destination.width;
            let v = (dest_y as f32 - destination.y) / destination.height;
            let source_x = (u * image.width as f32).floor() as i32;
            let source_y = (v * image.height as f32).floor() as i32;
            if source_x < 0
                || source_y < 0
                || source_x >= image.width as i32
                || source_y >= image.height as i32
            {
                continue;
            }

            let mut color = if shrink_x || shrink_y {
                let source_left = if shrink_x {
                    ((dest_x as f32 - destination.x) * source_per_dest_x)
                        .clamp(0.0, image.width as f32)
                } else {
                    source_x as f32
                };
                let source_right = if shrink_x {
                    (((dest_x + 1) as f32 - destination.x) * source_per_dest_x)
                        .clamp(0.0, image.width as f32)
                } else {
                    source_x as f32 + 1.0
                };
                let source_top = if shrink_y {
                    ((dest_y as f32 - destination.y) * source_per_dest_y)
                        .clamp(0.0, image.height as f32)
                } else {
                    source_y as f32
                };
                let source_bottom = if shrink_y {
                    (((dest_y + 1) as f32 - destination.y) * source_per_dest_y)
                        .clamp(0.0, image.height as f32)
                } else {
                    source_y as f32 + 1.0
                };
                reference_box_filter_sample(
                    image,
                    source_left,
                    source_top,
                    source_right,
                    source_bottom,
                )
            } else {
                let source_index = ((source_y as u32 * image.width + source_x as u32) * 4) as usize;
                Color {
                    r: image.pixels[source_index],
                    g: image.pixels[source_index + 1],
                    b: image.pixels[source_index + 2],
                    a: image.pixels[source_index + 3],
                }
            };
            if opacity < 1.0 {
                color.a = (f32::from(color.a) * opacity).round() as u8;
            }
            let dest_index = ((dest_y as u32 * canvas.width + dest_x as u32) * 4) as usize;
            blend_pixel(&mut canvas.pixels[dest_index..dest_index + 4], color);
        }
    }
}

fn baseline_case(
    name: &'static str,
    width: u32,
    height: u32,
    destination: Rect,
    clip: Option<Rect>,
    opacity: f32,
) -> BaselineCase {
    BaselineCase {
        name,
        image: patterned_image(width, height),
        destination,
        clip,
        opacity,
    }
}

fn baseline_cases() -> Vec<BaselineCase> {
    type Spec = (&'static str, u32, u32, Rect, Option<Rect>, f32);
    let specs: [Spec; 17] = [
        (
            "shrink_both_axes",
            9,
            7,
            rect(1.0, 1.0, 3.375, 2.625),
            None,
            1.0,
        ),
        ("shrink_x_only", 9, 3, rect(0.5, 0.25, 4.25, 6.5), None, 1.0),
        ("shrink_y_only", 3, 9, rect(0.25, 0.5, 6.5, 4.25), None, 1.0),
        ("nearest_expand", 3, 2, rect(1.0, 1.0, 7.25, 5.5), None, 1.0),
        ("one_to_one", 5, 4, rect(2.0, 3.0, 5.0, 4.0), None, 1.0),
        (
            "fractional_destination",
            7,
            5,
            rect(1.375, 0.625, 3.75, 2.4),
            None,
            1.0,
        ),
        (
            "negative_destination_origin",
            7,
            5,
            rect(-2.375, -1.625, 4.875, 3.5),
            None,
            1.0,
        ),
        (
            "partial_fractional_clip",
            9,
            7,
            rect(-0.75, 0.25, 6.5, 5.25),
            Some(rect(1.25, 1.75, 3.1, 2.2)),
            1.0,
        ),
        (
            "non_intersecting_clip",
            5,
            4,
            rect(1.0, 1.0, 4.0, 3.0),
            Some(rect(10.0, 9.0, 2.0, 1.0)),
            1.0,
        ),
        ("half_opacity", 7, 5, rect(0.5, 1.5, 3.75, 2.5), None, 0.5),
        ("zero_opacity", 7, 5, rect(0.5, 1.5, 3.75, 2.5), None, 0.0),
        (
            "nan_opacity",
            7,
            5,
            rect(0.5, 1.5, 3.75, 2.5),
            None,
            f32::NAN,
        ),
        (
            "zero_destination_width",
            4,
            3,
            rect(1.0, 1.0, 0.0, 2.0),
            None,
            1.0,
        ),
        (
            "zero_destination_height",
            4,
            3,
            rect(1.0, 1.0, 2.0, 0.0),
            None,
            1.0,
        ),
        (
            "invalid_negative_width",
            4,
            3,
            rect(1.0, 1.0, -2.0, 2.0),
            None,
            1.0,
        ),
        (
            "invalid_negative_height",
            4,
            3,
            rect(1.0, 1.0, 2.0, -2.0),
            None,
            1.0,
        ),
        (
            "zero_sized_image",
            0,
            0,
            rect(1.0, 1.0, 3.0, 2.0),
            None,
            1.0,
        ),
    ];
    let mut cases = specs
        .into_iter()
        .map(|(name, width, height, destination, clip, opacity)| {
            baseline_case(name, width, height, destination, clip, opacity)
        })
        .collect::<Vec<_>>();
    cases.push(BaselineCase {
        name: "premultiplied_alpha_shrink",
        image: premultiplied_alpha_image(),
        destination: rect(0.5, 0.5, 2.25, 1.25),
        clip: None,
        opacity: 1.0,
    });
    cases
}

#[test]
fn image_scaling_matches_frozen_reference_for_deterministic_matrix() {
    for case in baseline_cases() {
        let original = patterned_canvas(13, 11);
        let mut actual = original.clone();
        let mut expected = original;

        actual.draw_image_scaled_clipped_with_opacity(
            &case.image,
            case.destination,
            case.clip,
            case.opacity,
        );
        reference_draw_image_scaled_clipped_with_opacity(
            &mut expected,
            &case.image,
            case.destination,
            case.clip,
            case.opacity,
        );

        assert_eq!(actual.width(), expected.width(), "{} width", case.name);
        assert_eq!(actual.height(), expected.height(), "{} height", case.name);
        assert_eq!(actual.pixels(), expected.pixels(), "{} RGBA", case.name);
    }
}

#[derive(Clone, Copy)]
struct ManualFixture {
    name: &'static str,
    canvas_width: u32,
    canvas_height: u32,
    destination: Rect,
    clip: Option<Rect>,
}

fn pixel_checksum(pixels: &[u8]) -> u64 {
    pixels.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn render_manual_fixture(image: &Image, fixture: ManualFixture, reference: bool) -> Canvas {
    let mut canvas = patterned_canvas(fixture.canvas_width, fixture.canvas_height);
    if reference {
        reference_draw_image_scaled_clipped_with_opacity(
            &mut canvas,
            image,
            fixture.destination,
            fixture.clip,
            1.0,
        );
    } else {
        canvas.draw_image_scaled_clipped_with_opacity(
            image,
            fixture.destination,
            fixture.clip,
            1.0,
        );
    }
    canvas
}

fn measure_manual_path(
    image: &Image,
    fixture: ManualFixture,
    reference: bool,
    iterations: usize,
) -> (Duration, u64) {
    let mut elapsed = Duration::default();
    let mut checksum = 0x517c_c1b7_2722_0a95u64;
    for _ in 0..iterations {
        let mut canvas = patterned_canvas(fixture.canvas_width, fixture.canvas_height);
        let start = Instant::now();
        if reference {
            reference_draw_image_scaled_clipped_with_opacity(
                &mut canvas,
                image,
                fixture.destination,
                fixture.clip,
                1.0,
            );
        } else {
            canvas.draw_image_scaled_clipped_with_opacity(
                image,
                fixture.destination,
                fixture.clip,
                1.0,
            );
        }
        elapsed += start.elapsed();
        checksum = checksum.rotate_left(7) ^ pixel_checksum(black_box(canvas.pixels()));
    }
    (elapsed, black_box(checksum))
}

#[test]
#[ignore = "manual immutable A/B timing fixture; run explicitly in release mode"]
fn manual_image_scaling_reference_ab() {
    const WARMUPS: usize = 2;
    const SAMPLES: usize = 5;
    const ITERATIONS_PER_SAMPLE: usize = 2;

    let image = patterned_image(320, 180);
    let fixtures = [
        ManualFixture {
            name: "shrink_both_320x180_to_189.333x106.5",
            canvas_width: 224,
            canvas_height: 128,
            destination: rect(7.125, 5.25, 189.333, 106.5),
            clip: None,
        },
        ManualFixture {
            name: "shrink_x_only_320x180_to_189.333x180",
            canvas_width: 224,
            canvas_height: 196,
            destination: rect(7.125, 5.25, 189.333, 180.0),
            clip: Some(rect(11.5, 8.25, 177.75, 169.5)),
        },
        ManualFixture {
            name: "nearest_expand_320x180_to_416.5x234.75",
            canvas_width: 448,
            canvas_height: 256,
            destination: rect(6.25, 4.5, 416.5, 234.75),
            clip: None,
        },
    ];

    for fixture in fixtures {
        for _ in 0..WARMUPS {
            let actual = render_manual_fixture(&image, fixture, false);
            let expected = render_manual_fixture(&image, fixture, true);
            assert_eq!(
                actual.pixels(),
                expected.pixels(),
                "{} warmup RGBA",
                fixture.name
            );
        }

        let mut actual_samples = Vec::with_capacity(SAMPLES);
        let mut reference_samples = Vec::with_capacity(SAMPLES);
        let mut stable_checksum = None;
        for sample in 0..SAMPLES {
            let (actual, reference) = if sample % 2 == 0 {
                (
                    measure_manual_path(&image, fixture, false, ITERATIONS_PER_SAMPLE),
                    measure_manual_path(&image, fixture, true, ITERATIONS_PER_SAMPLE),
                )
            } else {
                let reference = measure_manual_path(&image, fixture, true, ITERATIONS_PER_SAMPLE);
                let actual = measure_manual_path(&image, fixture, false, ITERATIONS_PER_SAMPLE);
                (actual, reference)
            };
            assert_eq!(
                actual.1, reference.1,
                "{} sample {sample} checksum",
                fixture.name
            );
            if let Some(previous) = stable_checksum {
                assert_eq!(actual.1, previous, "{} stable checksum", fixture.name);
            }
            stable_checksum = Some(actual.1);
            actual_samples.push(actual.0.as_nanos());
            reference_samples.push(reference.0.as_nanos());
        }

        println!(
            "{} checksum={:#018x} actual_ns={actual_samples:?} reference_ns={reference_samples:?}",
            fixture.name,
            stable_checksum.unwrap()
        );
    }
}
