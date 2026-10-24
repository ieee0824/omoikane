use super::image_scale::{
    ImageScalePlan, MAX_IMAGE_SCALE_PLAN_PAYLOAD_BYTES, begin_image_scale_sample_trace,
    take_image_scale_plan_counts, take_image_scale_sample_trace,
};
use super::image_scale_baseline_tests::reference_draw_image_scaled_clipped_with_opacity;
use super::{Canvas, Color, Image};
use crate::layout::Rect;

fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn patterned_image(width: u32, height: u32) -> Image {
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            pixels.extend_from_slice(&[
                ((x * 37 + y * 11 + 3) % 256) as u8,
                ((x * 17 + y * 43 + 19) % 256) as u8,
                ((x * 71 + y * 5 + 97) % 256) as u8,
                [0, 1, 63, 127, 128, 191, 254, 255][((y * width + x) % 8) as usize],
            ]);
        }
    }
    Image::new(width, height, pixels).unwrap()
}

fn patterned_canvas(width: u32, height: u32) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    for y in 0..height {
        for x in 0..width {
            canvas.set_pixel(
                x,
                y,
                Color::rgba(
                    ((x * 7 + y * 29 + 13) % 256) as u8,
                    ((x * 31 + y * 3 + 23) % 256) as u8,
                    ((x * 5 + y * 47 + 101) % 256) as u8,
                    ((x * 41 + y * 19 + 53) % 256) as u8,
                ),
            );
        }
    }
    canvas
}

fn direct_axis_weights(
    destination: i32,
    destination_origin: f32,
    destination_extent: f32,
    source_extent: u32,
    source_per_destination: f32,
    shrink: bool,
) -> (u32, Vec<(u32, u64)>) {
    let normalized = (destination as f32 - destination_origin) / destination_extent;
    let nearest = (normalized * source_extent as f32).floor() as i32;
    if nearest < 0 || nearest >= source_extent as i32 {
        return (0, Vec::new());
    }

    let source_start = if shrink {
        ((destination as f32 - destination_origin) * source_per_destination)
            .clamp(0.0, source_extent as f32)
    } else {
        nearest as f32
    };
    let source_end = if shrink {
        (((destination + 1) as f32 - destination_origin) * source_per_destination)
            .clamp(0.0, source_extent as f32)
    } else {
        nearest as f32 + 1.0
    };
    let first_source = source_start.floor().max(0.0) as u32;
    let source_end_exclusive = source_end.ceil().min(source_extent as f32) as u32;
    let weights = (first_source..source_end_exclusive)
        .map(|source| {
            let weight = (source_end.min(source as f32 + 1.0) - source_start.max(source as f32))
                .max(0.0) as f64;
            (source, weight.to_bits())
        })
        .collect();
    (first_source, weights)
}

fn assert_axis_entry(
    axis: &str,
    destination: i32,
    expected: (u32, Vec<(u32, u64)>),
    actual: Option<(u32, &[f64])>,
) {
    let (actual_first, actual_weights) = actual.expect("destination is in the planned range");
    let actual = actual_weights
        .iter()
        .enumerate()
        .map(|(offset, weight)| (actual_first + offset as u32, weight.to_bits()))
        .collect::<Vec<_>>();
    assert_eq!(
        actual_first, expected.0,
        "{axis} destination {destination} first source"
    );
    assert_eq!(
        actual, expected.1,
        "{axis} destination {destination} source order and bits"
    );
}

#[test]
fn axis_plan_preserves_direct_formula_weight_bits_and_source_order() {
    let _ = take_image_scale_plan_counts();
    let x_range = 0..9;
    let y_range = 0..13;
    let x_origin = -0.625;
    let y_origin = 0.375;
    let destination_width = 7.25;
    let destination_height = 12.5;
    let source_width = 19;
    let source_height = 5;
    let source_per_destination_x = source_width as f32 / destination_width;
    let source_per_destination_y = source_height as f32 / destination_height;
    let plan = ImageScalePlan::try_new(
        x_range.clone(),
        y_range.clone(),
        x_origin,
        y_origin,
        destination_width,
        destination_height,
        source_width,
        source_height,
        source_per_destination_x,
        source_per_destination_y,
        true,
        false,
    )
    .expect("small plan must fit");

    for destination in x_range {
        let expected = direct_axis_weights(
            destination,
            x_origin,
            destination_width,
            source_width,
            source_per_destination_x,
            true,
        );
        assert_axis_entry(
            "x",
            destination,
            expected,
            plan.x_weighted_sources_for_test(destination),
        );
    }
    for destination in y_range {
        let expected = direct_axis_weights(
            destination,
            y_origin,
            destination_height,
            source_height,
            source_per_destination_y,
            false,
        );
        assert_axis_entry(
            "y",
            destination,
            expected,
            plan.y_weighted_sources_for_test(destination),
        );
    }
    assert_eq!(take_image_scale_plan_counts(), (1, 1));
}

#[test]
fn sampling_visits_the_cartesian_sources_with_y_outer_and_x_inner() {
    let _ = take_image_scale_plan_counts();
    let image = patterned_image(5, 5);
    let source_per_destination_x = 5.0 / 2.0;
    let source_per_destination_y = 5.0 / 3.0;
    let plan = ImageScalePlan::try_new(
        0..2,
        0..3,
        0.0,
        0.0,
        2.0,
        3.0,
        5,
        5,
        source_per_destination_x,
        source_per_destination_y,
        true,
        true,
    )
    .expect("small plan must fit");

    let (_, x_weights) = direct_axis_weights(0, 0.0, 2.0, 5, source_per_destination_x, true);
    let (_, y_weights) = direct_axis_weights(0, 0.0, 3.0, 5, source_per_destination_y, true);
    assert!(x_weights.len() > 1);
    assert!(y_weights.len() > 1);
    assert_ne!(y_weights.first().unwrap().1, y_weights.last().unwrap().1);
    let mut expected_trace = Vec::new();
    for &(source_y, y_weight_bits) in &y_weights {
        for &(source_x, x_weight_bits) in &x_weights {
            let weight = f64::from_bits(x_weight_bits) * f64::from_bits(y_weight_bits);
            expected_trace.push((source_y, source_x, weight.to_bits()));
        }
    }

    begin_image_scale_sample_trace();
    let _ = plan.sample(&image, 0, 0);
    assert_eq!(take_image_scale_sample_trace(), expected_trace);
    assert_eq!(take_image_scale_plan_counts(), (1, 1));
}

#[test]
fn plan_payload_is_bounded_and_oversized_30000_by_1_plan_is_rejected() {
    let _ = take_image_scale_plan_counts();
    let normal = ImageScalePlan::try_new(
        0..101,
        0..51,
        0.0,
        0.0,
        100.25,
        50.5,
        320,
        180,
        320.0 / 100.25,
        180.0 / 50.5,
        true,
        true,
    )
    .expect("normal plan must fit");
    assert!(
        normal.payload_capacity_bytes_for_test().unwrap() <= MAX_IMAGE_SCALE_PLAN_PAYLOAD_BYTES
    );

    let oversized = ImageScalePlan::try_new(
        0..30_000,
        0..1,
        0.0,
        0.0,
        30_000.0,
        1.0,
        60_000,
        1,
        2.0,
        1.0,
        true,
        false,
    );
    assert!(oversized.is_none());
    assert_eq!(take_image_scale_plan_counts(), (2, 1));
}

#[test]
fn canvas_reports_no_plan_for_nearest_and_one_successful_plan_for_shrink() {
    let image = patterned_image(2, 2);
    let _ = take_image_scale_plan_counts();

    let mut nearest = Canvas::new(6, 6);
    nearest.draw_image_scaled_clipped(&image, rect(1.0, 1.0, 4.0, 4.0), None);
    assert_eq!(take_image_scale_plan_counts(), (0, 0));

    let mut shrink = Canvas::new(2, 2);
    shrink.draw_image_scaled_clipped(&image, rect(0.0, 0.0, 1.0, 1.0), None);
    assert_eq!(take_image_scale_plan_counts(), (1, 1));
}

#[test]
fn cap_fallback_matches_frozen_reference_for_every_rgba_byte() {
    let image = patterned_image(60_000, 1);
    let destination = rect(0.0, 0.0, 30_000.0, 1.0);
    let original = patterned_canvas(30_000, 1);
    let mut actual = original.clone();
    let mut expected = original;
    let _ = take_image_scale_plan_counts();

    actual.draw_image_scaled_clipped_with_opacity(&image, destination, None, 0.5);
    reference_draw_image_scaled_clipped_with_opacity(&mut expected, &image, destination, None, 0.5);

    assert_eq!(take_image_scale_plan_counts(), (1, 0));
    assert_eq!(actual.pixels(), expected.pixels());
}
