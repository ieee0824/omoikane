use omoikane::html::TreeBuilder;
use omoikane::layout::Rect;
use omoikane::paint::{Canvas, render_document};

#[test]
fn text_shadow_firefox_fixture_renders_all_panels() {
    use base64::Engine;
    let fixture = std::path::Path::new("tests/fixtures/text-shadow/rendering.html");
    let font =
        base64::engine::general_purpose::STANDARD.encode(include_bytes!("fixtures/acid3/font.ttf"));
    // Local CSS font URLs are not materialized by the image-only base-path API.
    // Embed the same existing font Firefox reads from the original fixture.
    let source = std::fs::read_to_string(fixture)
        .unwrap()
        .replace("../acid3/font.ttf", &format!("data:font/ttf;base64,{font}"));
    let document = TreeBuilder::parse(&source).document();
    let canvas = omoikane::paint::render_document_with_base_path(
        &document,
        Rect {
            width: 480.0,
            height: 360.0,
            ..Rect::default()
        },
        fixture.parent().unwrap(),
    )
    .unwrap();
    let black = (0..160)
        .flat_map(|y| (0..120).map(move |x| (x, y)))
        .filter(|(x, y)| canvas.pixel(*x, *y) == Some(omoikane::paint::Color::rgb(0, 0, 0)))
        .count();
    assert!(
        black > 600,
        "Ahem must paint two solid squares, black pixels={black}"
    );
    for row in 0..2 {
        for column in 0..4 {
            let colored = (row * 180..row * 180 + 160)
                .flat_map(|y| (column * 120..column * 120 + 120).map(move |x| (x, y)))
                .filter(|(x, y)| {
                    let pixel = canvas.pixel(*x, *y).unwrap();
                    pixel.a > 0 && (pixel.r < 255 || pixel.g < 255 || pixel.b < 255)
                })
                .count();
            assert!(colored > 0, "empty fixture panel ({column},{row})");
        }
    }
    if let Some(output) = std::env::var_os("OMOIKANE_TEXT_SHADOW_IMAGES") {
        let output = std::path::PathBuf::from(output);
        std::fs::create_dir_all(&output).unwrap();
        std::fs::write(output.join("rendering.actual.png"), canvas.encode_png()).unwrap();
    }
}

fn render(body: &str) -> Canvas {
    let source = format!(
        "<!doctype html><style>body{{margin:0;background:white}} div{{position:absolute;left:30px;top:20px;font:24px sans-serif;white-space:pre}}</style>{body}"
    );
    let document = TreeBuilder::parse(&source).document();
    render_document(
        &document,
        Rect {
            width: 240.0,
            height: 160.0,
            ..Rect::default()
        },
    )
    .unwrap()
}

fn assert_same(actual: Canvas, expected: Canvas) {
    for (index, (a, b)) in actual
        .pixels()
        .chunks_exact(4)
        .zip(expected.pixels().chunks_exact(4))
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .take(8)
    {
        eprintln!(
            "pixel ({}, {}): actual={a:?}, expected={b:?}",
            index % actual.width() as usize,
            index / actual.width() as usize
        );
    }
    let mismatches = actual
        .pixels()
        .chunks_exact(4)
        .zip(expected.pixels().chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(mismatches, 0, "pixel mismatch");
}

#[test]
fn text_shadow_transparent_foreground_casts_glyph_and_decoration_shadow() {
    assert_same(
        render(
            "<div style='color:transparent;text-decoration:underline;text-shadow:red 10px 8px'>Shadow</div>",
        ),
        render("<div style='left:40px;top:28px;color:red;text-decoration:underline'>Shadow</div>"),
    );
}

#[test]
fn text_shadow_paints_form_control_value_above_its_background() {
    for tag in ["input", "textarea"] {
        let content = if tag == "input" { "value='Shadow'" } else { "" };
        let text = if tag == "textarea" { "Shadow" } else { "" };
        let actual = render(&format!(
            "<{tag} {content} style='position:absolute;left:20px;top:20px;width:160px;height:60px;background:white;font:24px sans-serif;color:transparent;text-shadow:red 10px 8px'>{text}</{tag}>"
        ));
        let red = actual
            .pixels()
            .chunks_exact(4)
            .filter(|p| p[0] > p[1] && p[0] > p[2] && p[3] > 0)
            .count();
        assert!(
            red > 20,
            "{tag} shadow must be visible above the control background: {red}"
        );
    }
}

#[test]
fn text_shadow_paints_inherited_list_marker_with_opacity_overflow() {
    let actual = render(
        "<ol style='position:absolute;left:40px;top:20px;font:24px sans-serif'><li style='color:transparent;opacity:.5;text-shadow:red 100px 40px'></li></ol>",
    );
    let red = actual
        .pixels()
        .chunks_exact(4)
        .enumerate()
        .filter(|(i, p)| {
            i % actual.width() as usize >= 100 && p[0] > p[1] && p[0] > p[2] && p[3] > 0
        })
        .count();
    assert!(
        red > 5,
        "marker shadow outside its original box must remain visible: {red}"
    );
}

#[test]
fn text_shadow_list_order_places_first_shadow_on_top() {
    assert_same(
        render(
            "<div style='color:transparent;text-shadow:red 10px 8px,blue 10px 8px'>Shadow</div>",
        ),
        render(
            "<div style='left:40px;top:28px;color:blue'>Shadow</div><div style='left:40px;top:28px;color:red'>Shadow</div>",
        ),
    );
}

#[test]
fn text_shadow_opacity_surface_contains_ink_outside_the_border_box() {
    assert_same(
        render("<div style='color:transparent;text-shadow:red 80px 40px;opacity:.5'>Shadow</div>"),
        render("<div style='left:110px;top:60px;color:red;opacity:.5'>Shadow</div>"),
    );
}

#[test]
fn text_shadow_vertical_writing_uses_the_same_glyph_and_decoration_geometry() {
    assert_same(
        render(
            "<div style='writing-mode:vertical-rl;color:transparent;text-decoration:underline;text-shadow:red 10px 8px'>Abc</div>",
        ),
        render(
            "<div style='left:40px;top:28px;writing-mode:vertical-rl;color:red;text-decoration:underline'>Abc</div>",
        ),
    );
}

#[test]
fn text_shadow_inherits_ancestor_clip_after_offsets() {
    assert_same(
        render(
            "<section style='position:absolute;width:110px;height:65px;overflow:hidden'><div style='color:transparent;text-shadow:red 40px 8px'>Shadow</div></section>",
        ),
        render(
            "<section style='position:absolute;width:110px;height:65px;overflow:hidden'><div style='left:70px;top:28px;color:red'>Shadow</div></section>",
        ),
    );
}

#[test]
fn text_shadow_is_transformed_with_the_ancestor() {
    assert_same(
        render(
            "<section style='position:absolute;left:35px;top:15px;transform:rotate(10deg);transform-origin:0 0'><div style='color:transparent;text-shadow:red 10px 8px'>Shadow</div></section>",
        ),
        render(
            "<section style='position:absolute;left:35px;top:15px;transform:rotate(10deg);transform-origin:0 0'><div style='left:40px;top:28px;color:red'>Shadow</div></section>",
        ),
    );
}

#[test]
fn text_shadow_blur_expands_ink_beyond_the_unblurred_glyph() {
    let sharp = render("<div style='color:transparent;text-shadow:red 0 0'>Shadow</div>");
    let blurred = render("<div style='color:transparent;text-shadow:red 0 0 8px'>Shadow</div>");
    let extents = |canvas: &Canvas| {
        let positions = canvas
            .pixels()
            .chunks_exact(4)
            .enumerate()
            .filter(|(_, p)| p[0] > 0 && p[1] == 0 && p[2] == 0 && p[3] > 0)
            .map(|(i, _)| (i % canvas.width() as usize, i / canvas.width() as usize))
            .collect::<Vec<_>>();
        (
            positions.iter().map(|p| p.0).min().unwrap(),
            positions.iter().map(|p| p.1).min().unwrap(),
            positions.iter().map(|p| p.0).max().unwrap(),
            positions.iter().map(|p| p.1).max().unwrap(),
        )
    };
    let a = extents(&sharp);
    let b = extents(&blurred);
    assert!(
        b.0 < a.0 && b.1 < a.1 && b.2 > a.2 && b.3 > a.3,
        "sharp={a:?}, blurred={b:?}"
    );
}

#[test]
fn text_shadow_outside_the_viewport_and_quantized_zero_blur_need_no_surface() {
    let transparent = render("<div style='color:transparent'>Shadow</div>");
    assert_same(
        render(
            "<div style='color:transparent;text-shadow:red 1000000000px 1000000000px'>Shadow</div>",
        ),
        transparent.clone(),
    );
    assert_same(
        render("<div style='color:transparent;text-shadow:red 0 0 1000000000px'>Shadow</div>"),
        transparent,
    );
}

#[test]
fn text_shadow_long_offscreen_source_is_clipped_to_the_destination() {
    let text = "Shadow".repeat(1000);
    let actual = render(&format!(
        "<div style='left:-100px;color:transparent;text-shadow:red 20px 8px'>{text}</div>"
    ));
    let expected = render(&format!(
        "<div style='left:-80px;top:28px;color:red'>{text}</div>"
    ));
    assert_same(actual, expected);
}

#[test]
fn text_shadow_ink_does_not_increase_scrollable_overflow() {
    let document = TreeBuilder::parse("<div id='box' style='width:100px;height:50px;overflow:auto'><span id='text'>Shadow</span></div>").document();
    let mut runtime = omoikane::js::JsRuntime::with_document(document).unwrap();
    runtime.set_viewport(240.0, 160.0);
    let result = runtime.eval("(() => { const box=document.getElementById('box'); const text=document.getElementById('text'); const before=box.scrollWidth+','+box.scrollHeight; text.style.textShadow='red 1000px 1000px 40px'; return before === box.scrollWidth+','+box.scrollHeight; })()").unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}
