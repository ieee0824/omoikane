use super::*;

fn command(json: &str) -> DrawCommand {
    serde_json::from_str(json).unwrap()
}

fn pixel(pixels: &[u8], width: usize, x: usize, y: usize) -> [u8; 4] {
    let index = (y * width + x) * 4;
    [
        pixels[index],
        pixels[index + 1],
        pixels[index + 2],
        pixels[index + 3],
    ]
}

fn run(pixels: &mut [u8], width: usize, height: usize, json: &str) {
    let mut surface = Surface {
        width,
        height,
        pixels,
    };
    assert!(draw(&mut surface, &command(json), None));
}

const LEFT_HALF: &str = r#"[{"points":[[0,0],[5,0],[5,10],[0,10]],"closed":true}]"#;

#[test]
fn fills_blend_with_global_alpha() {
    let mut pixels = vec![0; 10 * 10 * 4];
    run(
        &mut pixels,
        10,
        10,
        &format!(
            r#"{{"kind":"fill","paths":{LEFT_HALF},"paint":{{"type":"color","color":[255,0,0,255]}}}}"#
        ),
    );
    run(
        &mut pixels,
        10,
        10,
        r#"{"kind":"fill","paths":[{"points":[[0,0],[10,0],[10,10],[0,10]]}],
            "alpha":0.5,"paint":{"type":"color","color":[0,0,255,255]}}"#,
    );
    assert_eq!(pixel(&pixels, 10, 2, 2), [128, 0, 128, 255]);
    assert_eq!(pixel(&pixels, 10, 7, 2), [0, 0, 255, 128]);
}

#[test]
fn unbounded_operators_clear_outside_the_shape_but_inside_the_clip() {
    let mut pixels = vec![255; 10 * 10 * 4];
    run(
        &mut pixels,
        10,
        10,
        &format!(
            r#"{{"kind":"fill","paths":[{{"points":[[0,0],[2,0],[2,2],[0,2]]}}],
                "composite":"copy","clip":[{{"paths":{LEFT_HALF}}}],
                "paint":{{"type":"color","color":[0,255,0,255]}}}}"#
        ),
    );
    assert_eq!(pixel(&pixels, 10, 1, 1), [0, 255, 0, 255]);
    assert_eq!(pixel(&pixels, 10, 3, 3), [0, 0, 0, 0]);
    assert_eq!(pixel(&pixels, 10, 7, 7), [255, 255, 255, 255]);
}

#[test]
fn strokes_use_the_user_space_line_width() {
    let mut pixels = vec![0; 20 * 20 * 4];
    // A 1px-wide line scaled by 4 horizontally is 4 device pixels wide.
    run(
        &mut pixels,
        20,
        20,
        r#"{"kind":"stroke","paths":[{"points":[[2.5,0],[2.5,20]]}],
            "transform":[4,0,0,1,0,0],"stroke":{"width":1},
            "paint":{"type":"color","color":[0,0,0,255]}}"#,
    );
    assert_eq!(pixel(&pixels, 20, 8, 5)[3], 255);
    assert_eq!(pixel(&pixels, 20, 11, 5)[3], 255);
    assert_eq!(pixel(&pixels, 20, 12, 5)[3], 0);
}

#[test]
fn shadows_draw_under_the_shape() {
    let mut pixels = vec![0; 20 * 20 * 4];
    run(
        &mut pixels,
        20,
        20,
        r#"{"kind":"fill","paths":[{"points":[[0,0],[10,0],[10,10],[0,10]]}],
            "shadow":{"color":[255,0,0,255],"offsetX":5,"offsetY":5},
            "paint":{"type":"color","color":[0,255,0,255]}}"#,
    );
    assert_eq!(pixel(&pixels, 20, 2, 2), [0, 255, 0, 255]);
    assert_eq!(pixel(&pixels, 20, 12, 12), [255, 0, 0, 255]);
    assert_eq!(pixel(&pixels, 20, 16, 16), [0, 0, 0, 0]);
}

#[test]
fn clear_respects_the_clip() {
    let mut pixels = vec![255; 10 * 10 * 4];
    run(
        &mut pixels,
        10,
        10,
        &format!(
            r#"{{"kind":"clear","paths":[{{"points":[[0,0],[10,0],[10,10],[0,10]]}}],
                "clip":[{{"paths":{LEFT_HALF}}}]}}"#
        ),
    );
    assert_eq!(pixel(&pixels, 10, 2, 2), [0, 0, 0, 0]);
    assert_eq!(pixel(&pixels, 10, 7, 2), [255, 255, 255, 255]);
}

#[test]
fn point_queries_follow_fill_and_stroke_geometry() {
    let square: Vec<SubPath> = serde_json::from_str(LEFT_HALF).unwrap();
    assert!(path_contains(&square, FillRule::NonZero, [5.0, 5.0]));
    assert!(!path_contains(&square, FillRule::NonZero, [5.1, 5.0]));
    assert!(!path_contains(&square, FillRule::NonZero, [f64::NAN, 5.0]));
    let style: StrokeStyle = serde_json::from_str(r#"{"width":2}"#).unwrap();
    assert!(stroke_contains(
        &square,
        Matrix::identity(),
        &style,
        [5.9, 5.0]
    ));
    assert!(!stroke_contains(
        &square,
        Matrix::identity(),
        &style,
        [2.5, 5.0]
    ));
}
