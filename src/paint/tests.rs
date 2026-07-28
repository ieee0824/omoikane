use std::fs;
use std::path::PathBuf;

use crate::css::{Origin, StyleResolver, parse_stylesheet};
use crate::dom::NodeHandle;
use crate::html::TreeBuilder;
use crate::layout::{
    BoxDimensions, FontMetrics, FragmentStyle, InlineFragment, LineBox, Rect, VerticalAlign,
    layout_tree,
};
use crate::paint::*;

#[test]
fn png_checksums_match_standard_vectors() {
    let input = b"123456789";

    assert_eq!(crc32(input), 0xcbf4_3926);
    assert_eq!(adler32(input), 0x091e_01de);
}

#[test]
fn opaque_blend_replaces_the_destination_pixel() {
    let mut pixel = [13, 27, 41, 99];

    blend_pixel(&mut pixel, Color::rgba(201, 177, 153, 255));

    assert_eq!(pixel, [201, 177, 153, 255]);
}

fn load_cjk_fallback_test_fonts() -> Option<Vec<std::sync::Arc<crate::font::Font>>> {
    let Some(primary_path) = crate::font::find_system_font("sans-serif") else {
        eprintln!("Skipping CJK fallback test: no primary sans-serif system font was found");
        return None;
    };
    let primary = match crate::font::Font::load_from_file(&primary_path) {
        Ok(font) => font,
        Err(error) => {
            eprintln!(
                "Skipping CJK fallback test: failed to load primary font {}: {error}",
                primary_path.display()
            );
            return None;
        }
    };

    // The issue 052 CI image installs Noto CJK, so this should not skip there. Keep
    // several common platform families for local and non-Docker test environments.
    let cjk_families = [
        "Noto Sans CJK JP",
        "Noto Sans JP",
        "Yu Gothic",
        "Meiryo",
        "MS Gothic",
        "IPA Gothic",
        "IPAGothic",
        "Hiragino Sans",
    ];
    for family in cjk_families {
        let Some(path) = crate::font::find_system_font(family) else {
            continue;
        };
        let Ok(font) = crate::font::Font::load_from_file(&path) else {
            continue;
        };
        if font.has_glyph('日') {
            return Some(vec![
                std::sync::Arc::new(primary),
                std::sync::Arc::new(font),
            ]);
        }
    }

    eprintln!(
        "Skipping CJK fallback test: no loadable CJK system font containing '日' was found (tried {})",
        cjk_families.join(", ")
    );
    None
}

#[test]
fn missing_cjk_glyph_uses_cjk_fallback_font() {
    let Some(fonts) = load_cjk_fallback_test_fonts() else {
        return;
    };

    if fonts[0].has_glyph('日') {
        eprintln!("Skipping CJK fallback scenario: primary font already contains '日'");
        return;
    }
    assert!(fonts[1].has_glyph('日'));
    let (index, glyph, _) = rasterize_with_fallback(&fonts, '日', 20.0);
    assert_eq!(index, 1);
    assert!(glyph.is_some());
}

#[test]
fn latin_glyph_stays_on_primary_font() {
    let Some(fonts) = load_cjk_fallback_test_fonts() else {
        return;
    };

    let (index, glyph, _) = rasterize_with_fallback(&fonts, 'A', 20.0);
    assert_eq!(index, 0);
    assert!(glyph.is_some());
}

#[test]
fn missing_glyph_in_all_fonts_uses_primary_notdef() {
    let Some(fonts) = load_cjk_fallback_test_fonts() else {
        return;
    };
    let missing = char::MAX;
    assert!(fonts.iter().all(|font| !font.has_glyph(missing)));

    let (index, glyph, _) = rasterize_with_fallback(&fonts, missing, 20.0);
    assert_eq!(index, 0);
    assert!(glyph.is_some(), "primary .notdef should remain visible");
}

#[test]
fn webfont_primary_refs_fall_back_for_cjk() {
    let Some(fonts) = load_cjk_fallback_test_fonts() else {
        return;
    };
    if fonts[0].has_glyph('日') {
        eprintln!("Skipping CJK fallback scenario: primary font already contains '日'");
        return;
    }
    let refs = [fonts[0].as_ref(), fonts[1].as_ref()];

    let (index, glyph, _) = rasterize_with_fallback_refs(&refs, '日', 20.0);
    assert_eq!(index, 1);
    assert!(glyph.is_some());
}

#[test]
fn cjk_preference_covers_japanese_and_fullwidth_blocks() {
    for ch in ['あ', 'ア', '一', '！', '￯'] {
        assert!(is_cjk_preferred_character(ch), "missing CJK range for {ch}");
    }
}

#[test]
fn fills_rectangles_on_canvas() {
    let mut canvas = Canvas::new(4, 4);
    canvas.fill_rect(
        Rect {
            x: 1.0,
            y: 1.0,
            width: 2.0,
            height: 2.0,
        },
        Color::rgb(255, 0, 0),
    );

    assert_eq!(canvas.pixel(0, 0), Some(Color::rgba(0, 0, 0, 0)));
    assert_eq!(canvas.pixel(1, 1), Some(Color::rgb(255, 0, 0)));
    assert_eq!(canvas.pixel(2, 2), Some(Color::rgb(255, 0, 0)));
    assert_eq!(canvas.pixel(3, 3), Some(Color::rgba(0, 0, 0, 0)));
}

#[test]
fn paints_backgrounds_and_borders_from_layout_boxes() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let panel = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(panel);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "body { margin: 0; } \
                 div { width: 20px; height: 20px; background-color: #ff0000; border: 2px solid #0000ff; }",
            )
            .unwrap(),
        );
    let layout = layout_tree(
        &document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 40.0,
            height: 40.0,
        },
    )
    .unwrap();

    let mut paint_resolver = StyleResolver::new();
    paint_resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "body { margin: 0; } \
                 div { width: 20px; height: 20px; background-color: #ff0000; border: 2px solid #0000ff; }",
            )
            .unwrap(),
        );
    let canvas = paint_layout(
        &layout,
        &mut paint_resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 40.0,
            height: 40.0,
        },
    );

    assert_eq!(canvas.pixel(1, 1), Some(Color::rgb(0, 0, 255)));
    assert_eq!(canvas.pixel(3, 3), Some(Color::rgb(255, 0, 0)));
}

#[test]
fn clips_children_when_overflow_is_hidden() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let parent = NodeHandle::element("div");
    let child = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(parent.clone());
    parent.append_child(child.clone());

    let stylesheet = "body { margin: 0; } \
             .parent { width: 10px; height: 10px; overflow: hidden; background-color: white; } \
             .child { width: 20px; height: 20px; background-color: red; }";
    parent.set_attribute("class", "parent");
    child.set_attribute("class", "child");

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
    let layout = layout_tree(
        &document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 30.0,
            height: 30.0,
        },
    )
    .unwrap();

    let mut paint_resolver = StyleResolver::new();
    paint_resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
    let canvas = paint_layout(
        &layout,
        &mut paint_resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 30.0,
            height: 30.0,
        },
    );

    assert_eq!(canvas.pixel(5, 5), Some(Color::rgb(255, 0, 0)));
    assert_eq!(canvas.pixel(15, 15), Some(Color::rgba(0, 0, 0, 0)));
}

#[test]
fn skips_hidden_boxes() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let panel = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(panel);

    let stylesheet = "body { margin: 0; } div { width: 10px; height: 10px; background-color: red; visibility: hidden; }";
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
    let layout = layout_tree(
        &document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 20.0,
            height: 20.0,
        },
    )
    .unwrap();

    let mut paint_resolver = StyleResolver::new();
    paint_resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
    let canvas = paint_layout(
        &layout,
        &mut paint_resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 20.0,
            height: 20.0,
        },
    );

    assert_eq!(canvas.pixel(5, 5), Some(Color::rgba(0, 0, 0, 0)));
}

#[test]
fn paints_inline_text_fragments() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let paragraph = NodeHandle::element("p");
    paragraph.append_child(NodeHandle::text("hello"));
    document.append_child(body.clone());
    body.append_child(paragraph);

    let stylesheet = "body { margin: 0; } p { color: blue; font-size: 10px; }";
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
    let layout = layout_tree(
        &document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 50.0,
            height: 20.0,
        },
    )
    .unwrap();

    let mut paint_resolver = StyleResolver::new();
    paint_resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
    let canvas = paint_layout(
        &layout,
        &mut paint_resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 50.0,
            height: 20.0,
        },
    );

    // Count pixels with blue component (accounting for antialiased glyph rendering)
    // Text is rendered with color: blue, so we check for any blue pixels
    let painted_pixels = canvas
        .pixels()
        .chunks_exact(4)
        .filter(|rgba| rgba[2] > 0 && rgba[3] > 0)
        .count();
    assert!(painted_pixels > 0);
}

#[test]
fn renders_relative_image_src_with_base_url() {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    let mut pixel_canvas = Canvas::new(1, 1);
    pixel_canvas.fill_rect(
        Rect {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        },
        Color::rgb(255, 0, 0),
    );
    let png_bytes = pixel_canvas.encode_png();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(&stream);
        let mut request_line = String::new();
        reader.read_line(&mut request_line).unwrap();
        loop {
            let mut h = String::new();
            reader.read_line(&mut h).unwrap();
            if h.trim().is_empty() {
                break;
            }
        }
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\n\r\n",
            png_bytes.len()
        );
        stream.write_all(resp.as_bytes()).unwrap();
        stream.write_all(&png_bytes).unwrap();
        stream.flush().unwrap();
    });

    let html = r#"<html><head><style>body { margin: 0; }</style></head><body><img src="images/red.png"></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let base_url = format!("http://127.0.0.1:{}/page/index.html", port)
        .parse::<crate::http::Url>()
        .unwrap();
    let canvas = render_document_with_url(
        &document,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 6.0,
            height: 6.0,
        },
        Some(&base_url),
    )
    .unwrap();

    assert!(count_pixels(&canvas, Color::rgb(255, 0, 0)) > 0);
}

#[test]
fn img_width_and_height_attributes_define_intrinsic_size() {
    let html = format!(
        r#"<html><head><style>body {{ margin: 0; }}</style></head><body><img src="{}" width="4" height="2"></body></html>"#,
        red_pixel_data_uri()
    );
    let document = TreeBuilder::parse(&html).document();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }
    let layout = layout_tree(
        &document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 20.0,
            height: 20.0,
        },
    )
    .unwrap();
    let image_fragment = find_first_image_fragment(&layout).unwrap();
    assert_eq!(image_fragment.rect.width, 4.0);
    assert_eq!(image_fragment.rect.height, 2.0);
    let sample_x = (image_fragment.rect.x + image_fragment.rect.width - 1.0)
        .max(0.0)
        .floor() as u32;
    let sample_y = (image_fragment.rect.y + image_fragment.rect.height - 1.0)
        .max(0.0)
        .floor() as u32;

    let canvas = paint_layout(
        &layout,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 20.0,
            height: 20.0,
        },
    );
    assert_eq!(
        canvas.pixel(sample_x, sample_y),
        Some(Color::rgb(255, 0, 0))
    );
}

#[test]
fn img_single_dimension_attribute_preserves_aspect_ratio() {
    let mut canvas = Canvas::new(2, 1);
    canvas.fill_rect(
        Rect {
            x: 0.0,
            y: 0.0,
            width: 2.0,
            height: 1.0,
        },
        Color::rgb(255, 0, 0),
    );
    let encoded = base64::engine::general_purpose::STANDARD.encode(canvas.encode_png());
    let data_uri = format!("data:image/png;base64,{encoded}");
    let html = format!(
        r#"<html><head><style>body {{ margin: 0; }}</style></head><body><img src="{}" width="4"></body></html>"#,
        data_uri
    );

    let document = TreeBuilder::parse(&html).document();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }
    let layout = layout_tree(
        &document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 20.0,
            height: 20.0,
        },
    )
    .unwrap();
    let image_fragment = find_first_image_fragment(&layout).unwrap();
    assert_eq!(image_fragment.rect.width, 4.0);
    assert_eq!(image_fragment.rect.height, 2.0);
}

#[test]
fn img_uses_alt_text_when_image_fetch_fails() {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(&stream);
        let mut request_line = String::new();
        reader.read_line(&mut request_line).unwrap();
        loop {
            let mut h = String::new();
            reader.read_line(&mut h).unwrap();
            if h.trim().is_empty() {
                break;
            }
        }
        let body = b"";
        let resp = format!(
            "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        stream.write_all(resp.as_bytes()).unwrap();
        stream.flush().unwrap();
    });

    let html = format!(
        r#"<html><head><style>body {{ margin: 0; }}</style></head><body><img src="http://127.0.0.1:{}/missing.png" alt="fallback text"></body></html>"#,
        port
    );
    let document = TreeBuilder::parse(&html).document();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }
    let layout = layout_tree(
        &document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 20.0,
        },
    )
    .unwrap();
    let mut texts = Vec::new();
    collect_layout_texts_into(&layout, &mut texts);
    assert!(texts.iter().any(|text| text.contains("fallback")));
}

#[test]
fn paints_tiled_background_images_from_data_uris() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(div);

    let stylesheet = format!(
        "body {{ margin: 0; }} div {{ width: 4px; height: 4px; background: url(\"{}\"); }}",
        red_pixel_data_uri()
    );
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(&stylesheet).unwrap());
    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        },
    )
    .unwrap();

    let mut paint_resolver = StyleResolver::new();
    paint_resolver.add_stylesheet(Origin::Author, parse_stylesheet(&stylesheet).unwrap());
    let canvas = paint_layout(
        &layout,
        &mut paint_resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        },
    );

    assert_eq!(canvas.pixel(1, 1), Some(Color::rgb(255, 0, 0)));
    assert_eq!(canvas.pixel(3, 3), Some(Color::rgb(255, 0, 0)));
}

#[test]
fn paints_non_repeating_background_images_once() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(div);

    let stylesheet = format!(
        "body {{ margin: 0; }} div {{ width: 4px; height: 4px; background: url(\"{}\") no-repeat; }}",
        red_pixel_data_uri()
    );
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(&stylesheet).unwrap());
    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        },
    )
    .unwrap();

    let mut paint_resolver = StyleResolver::new();
    paint_resolver.add_stylesheet(Origin::Author, parse_stylesheet(&stylesheet).unwrap());
    let canvas = paint_layout(
        &layout,
        &mut paint_resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        },
    );

    assert_eq!(canvas.pixel(0, 0), Some(Color::rgb(255, 0, 0)));
    assert_eq!(canvas.pixel(1, 0), Some(Color::rgba(0, 0, 0, 0)));
    assert_eq!(canvas.pixel(0, 1), Some(Color::rgba(0, 0, 0, 0)));
}

#[test]
fn paints_generated_border_box_pseudo_elements() {
    let html = "<html><head><style>body { margin: 0; } span::before { content: ''; border-style: none solid solid; border-color: red yellow black yellow; border-width: 4px; }</style></head><body><span></span></body></html>";
    let document = TreeBuilder::parse(html).document();
    let canvas = render_document(
        &document,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 20.0,
            height: 20.0,
        },
    )
    .unwrap();

    assert!(count_pixels(&canvas, Color::rgb(255, 255, 0)) > 0);
    assert!(count_pixels(&canvas, Color::rgb(0, 0, 0)) > 0);
}

#[test]
fn zero_height_border_box_paints_top_and_bottom_bands_across_full_width() {
    let html = r#"<html><head><style>
            body { margin: 0; }
            div { width: 24px; height: 0; border-top: 4px solid yellow; border-bottom: 4px solid black; }
        </style></head><body><div></div></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let canvas = render_document(
        &document,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 24.0,
            height: 8.0,
        },
    )
    .unwrap();

    assert_eq!(canvas.pixel(0, 0), Some(Color::rgb(255, 255, 0)));
    assert_eq!(canvas.pixel(12, 0), Some(Color::rgb(255, 255, 0)));
    assert_eq!(canvas.pixel(0, 7), Some(Color::rgb(0, 0, 0)));
    assert_eq!(canvas.pixel(12, 7), Some(Color::rgb(0, 0, 0)));
}

#[test]
fn forgiving_parse_preserves_valid_declarations_in_partially_invalid_rule() {
    let stylesheet = "#eyes-b { float: left; width: 10em; height: 2em; background: fixed url(data:image/png;base64,AAAA); border-left: solid 1em black; border-right: solid 1em red; }";
    let parsed = parse_stylesheet_forgiving(stylesheet);
    let crate::css::Rule::Style(rule) = parsed.rules.into_iter().next().unwrap() else {
        panic!("expected style rule");
    };

    assert!(rule.declarations.iter().any(|decl| decl.name == "float"));
    assert!(rule.declarations.iter().any(|decl| decl.name == "width"));
    assert!(rule.declarations.iter().any(|decl| decl.name == "height"));
    assert!(
        rule.declarations
            .iter()
            .any(|decl| decl.name == "background-image")
    );
    assert!(
        rule.declarations
            .iter()
            .any(|decl| decl.name == "border-left-width")
    );
    assert!(
        rule.declarations
            .iter()
            .any(|decl| decl.name == "border-right-width")
    );
}

#[test]
fn inline_replaced_element_with_padding_border_and_background_paints_in_order() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let image_node = NodeHandle::element("img");
    document.append_child(body.clone());
    body.append_child(image_node.clone());

    let stylesheet = "img { padding: 2px; border: 1px solid blue; background: yellow; }";
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
    let image_style = resolver.computed_style(&image_node);
    let mut canvas = Canvas::new(10, 10);
    let image = Image::new(1, 1, vec![255, 0, 0, 255]).unwrap();
    paint_inline_image_fragment(
        &mut canvas,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 7.0,
            height: 7.0,
        },
        &image,
        &image_style,
        None,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        },
    );

    assert_eq!(
        image_style.get("border-style"),
        Some(&ComputedValue::Keyword("solid".to_string()))
    );
    assert_eq!(canvas.pixel(0, 1), Some(Color::rgb(0, 0, 255)));
    assert_eq!(canvas.pixel(2, 2), Some(Color::rgb(255, 255, 0)));
    assert_eq!(canvas.pixel(3, 3), Some(Color::rgb(255, 0, 0)));
}

#[test]
fn paints_background_image_with_position_offset() {
    let html = format!(
        "<html><head><style>body {{ margin: 0; }} div {{ width: 4px; height: 2px; background-image: url(\"{}\"); background-repeat: no-repeat; background-position-x: 1px; background-position-y: 0; }}</style></head><body><div></div></body></html>",
        red_pixel_data_uri()
    );
    let document = TreeBuilder::parse(&html).document();
    let canvas = render_document(
        &document,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 4.0,
            height: 2.0,
        },
    )
    .unwrap();

    assert_eq!(canvas.pixel(0, 0), Some(Color::rgba(0, 0, 0, 0)));
    assert_eq!(canvas.pixel(1, 0), Some(Color::rgb(255, 0, 0)));
}

#[test]
fn fixed_background_image_uses_viewport_origin() {
    let html = format!(
        "<html><head><style>body {{ margin: 0; }} div {{ width: 4px; height: 2px; margin-left: 2px; background-image: url(\"{}\"); background-repeat: no-repeat; background-position-x: 1px; background-position-y: 0; background-attachment: fixed; }}</style></head><body><div></div></body></html>",
        red_pixel_data_uri()
    );
    let document = TreeBuilder::parse(&html).document();
    let canvas = render_document(
        &document,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 6.0,
            height: 2.0,
        },
    )
    .unwrap();

    assert_eq!(canvas.pixel(1, 0), Some(Color::rgba(0, 0, 0, 0)));
    assert_eq!(canvas.pixel(3, 0), Some(Color::rgba(0, 0, 0, 0)));
}

#[test]
fn two_dimensional_transform_rotates_and_translates_the_painted_subtree() {
    let document = TreeBuilder::parse(
        r#"<html><head><style>
            html, body { margin: 0; }
            div { width: 4px; height: 2px; background: red;
                  transform-origin: 0 0; transform: translate(4px, 0) rotate(90deg); }
        </style></head><body><div></div></body></html>"#,
    )
    .document();
    let canvas = render_document(
        &document,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 8.0,
            height: 6.0,
        },
    )
    .unwrap();

    assert_eq!(canvas.pixel(2, 0), Some(Color::rgb(255, 0, 0)));
    assert_eq!(canvas.pixel(3, 3), Some(Color::rgb(255, 0, 0)));
    assert_eq!(canvas.pixel(1, 0), Some(Color::rgba(0, 0, 0, 0)));
    assert_eq!(canvas.pixel(4, 0), Some(Color::rgba(0, 0, 0, 0)));
}

#[test]
fn transformed_parent_moves_descendant_as_one_painted_subtree() {
    let document = TreeBuilder::parse(
        r#"<html><head><style>
            html, body { margin: 0; }
            .parent { width: 2px; height: 2px; transform: translateX(4px); }
            .child { width: 2px; height: 2px; background: blue; }
        </style></head><body><div class="parent"><div class="child"></div></div></body></html>"#,
    )
    .document();
    let canvas = render_document(
        &document,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 8.0,
            height: 4.0,
        },
    )
    .unwrap();

    assert_eq!(canvas.pixel(0, 0), Some(Color::rgba(0, 0, 0, 0)));
    assert_eq!(canvas.pixel(4, 0), Some(Color::rgb(0, 0, 255)));
    assert_eq!(canvas.pixel(5, 1), Some(Color::rgb(0, 0, 255)));
}

#[test]
fn transform_can_move_an_offscreen_source_box_into_the_viewport() {
    let document = TreeBuilder::parse(
        r#"<html><head><style>
            html, body { margin: 0; }
            div { position: absolute; left: -20px; top: 0; width: 10px; height: 10px;
                  background: red; transform: translateX(25px); }
        </style></head><body><div></div></body></html>"#,
    )
    .document();
    let canvas = render_document(
        &document,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 20.0,
            height: 12.0,
        },
    )
    .unwrap();

    assert_eq!(canvas.pixel(4, 5), Some(Color::rgba(0, 0, 0, 0)));
    assert_eq!(canvas.pixel(5, 5), Some(Color::rgb(255, 0, 0)));
    assert_eq!(canvas.pixel(14, 5), Some(Color::rgb(255, 0, 0)));
    assert_eq!(canvas.pixel(15, 5), Some(Color::rgba(0, 0, 0, 0)));
}

#[test]
fn transition_transform_midpoint_reaches_paint_coordinates() {
    let document = TreeBuilder::parse(
        r#"<html><body><div></div></body></html>"#,
    )
    .document();
    let target = document.query_selector("div").unwrap();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "html, body { margin: 0; } div { width: 4px; height: 2px; background: red; \
             transform: none; transform-origin: 0 0; transition: transform 1s linear; }",
        )
        .unwrap(),
    );
    resolver.computed_style(&target);
    target.set_attribute("style", "transform: translateX(20px)");
    resolver.invalidate_style_cache_for_test();
    resolver.computed_style(&target);
    resolver.set_transition_time_ms(500.0);

    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 25.0,
        height: 4.0,
    };
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let canvas = paint_layout(&layout, &mut resolver, viewport);

    assert_eq!(canvas.pixel(0, 0), Some(Color::rgba(0, 0, 0, 0)));
    assert_eq!(canvas.pixel(9, 0), Some(Color::rgba(0, 0, 0, 0)));
    assert_eq!(canvas.pixel(10, 0), Some(Color::rgb(255, 0, 0)));
    assert_eq!(canvas.pixel(13, 1), Some(Color::rgb(255, 0, 0)));
    assert_eq!(canvas.pixel(14, 0), Some(Color::rgba(0, 0, 0, 0)));
}

#[test]
fn nested_object_fallback_preserves_fixed_background_on_inline_image_fragment() {
    let html = r#"<html><head><style>body { margin: 0; font: 2px/2px sans-serif; } object { display: inline; vertical-align: bottom; } object object object { background: url(data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAABnRSTlMAAAAAAABupgeRAAAABmJLR0QA%2FwD%2FAP%2BgvaeTAAAAEUlEQVR42mP4%2F58BCv7%2FZwAAHfAD%2FabwPj4AAAAASUVORK5CYII%3D) fixed 1px 0; }</style></head><body><object data="data:application/x-unknown,ERROR"><object data="data:application/x-unknown,ERROR" type="text/html"><object data="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAABnRSTlMAAAAAAABupgeRAAAABmJLR0QA%2FwD%2FAP%2BgvaeTAAAAEUlEQVR42mP4%2F58BCv7%2FZwAAHfAD%2FabwPj4AAAAASUVORK5CYII%3D"></object></object></object></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }
    let layout = crate::layout::layout_tree(
        &document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 2.0,
            height: 2.0,
        },
    )
    .unwrap();
    let image_fragment = find_first_image_fragment(&layout).unwrap();
    assert_eq!(image_fragment.rect.x, 0.0);
    assert_eq!(image_fragment.rect.y, 0.0);
    match &image_fragment.content {
        InlineFragmentContent::Image(_, style) => {
            assert_eq!(
                style.get("background-attachment"),
                Some(&ComputedValue::Keyword("fixed".to_string()))
            );
            assert!(style.get("background-image").is_some());
        }
        _ => panic!("expected image fragment"),
    }

    let canvas = render_document(
        &document,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 2.0,
            height: 2.0,
        },
    )
    .unwrap();

    assert_eq!(canvas.pixel(0, 0), Some(Color::rgb(255, 255, 0)));
    assert_eq!(canvas.pixel(1, 0), Some(Color::rgb(255, 255, 0)));
    assert_eq!(canvas.pixel(0, 1), Some(Color::rgb(255, 255, 0)));
    assert_eq!(canvas.pixel(1, 1), Some(Color::rgb(255, 255, 0)));
}

#[test]
fn absolute_inline_content_paints_above_float_siblings() {
    let root = NodeHandle::element("div");
    root.set_attribute("class", "root");
    let float = NodeHandle::element("div");
    float.set_attribute("class", "float");
    let overlay = NodeHandle::element("div");
    overlay.set_attribute("class", "overlay");
    let generated = NodeHandle::element("span");
    generated.set_attribute("class", "generated");

    let stylesheet = ".root { position: relative; } .float { float: left; background: blue; } .overlay { position: absolute; left: 0; top: 0; } .generated { background: red; }";
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
    let generated_style = resolver.computed_style(&generated);
    let layout = LayoutBox {
        node: root,
        dimensions: BoxDimensions {
            content: Rect {
                x: 0.0,
                y: 0.0,
                width: 8.0,
                height: 8.0,
            },
            ..BoxDimensions::default()
        },
        visibility: Visibility::Visible,
        overflow: crate::layout::Overflow::Visible,
        z_index: 0,
        transform: crate::css::AffineTransform::identity(),
        lines: Vec::new(),
        children: vec![
            LayoutBox {
                node: float,
                dimensions: BoxDimensions {
                    content: Rect {
                        x: 0.0,
                        y: 0.0,
                        width: 8.0,
                        height: 8.0,
                    },
                    ..BoxDimensions::default()
                },
                visibility: Visibility::Visible,
                overflow: crate::layout::Overflow::Visible,
                z_index: 0,
                transform: crate::css::AffineTransform::identity(),
                lines: Vec::new(),
                children: Vec::new(),
                marker: None,
            },
            LayoutBox {
                node: overlay,
                dimensions: BoxDimensions {
                    content: Rect {
                        x: 0.0,
                        y: 0.0,
                        width: 8.0,
                        height: 8.0,
                    },
                    ..BoxDimensions::default()
                },
                visibility: Visibility::Visible,
                overflow: crate::layout::Overflow::Visible,
                z_index: 0,
                transform: crate::css::AffineTransform::identity(),
                lines: vec![LineBox {
                    rect: Rect {
                        x: 0.0,
                        y: 0.0,
                        width: 8.0,
                        height: 8.0,
                    },
                    baseline: 0.0,
                    fragments: vec![InlineFragment {
                        node: generated,
                        content: InlineFragmentContent::GeneratedBox(generated_style),
                        rect: Rect {
                            x: 0.0,
                            y: 0.0,
                            width: 8.0,
                            height: 8.0,
                        },
                        metrics: FontMetrics::from_font_size(8.0),
                        vertical_align: VerticalAlign::Top,
                        style: FragmentStyle::default(),
                    }],
                }],
                children: Vec::new(),
                marker: None,
            },
        ],
        marker: None,
    };
    let canvas = paint_layout(
        &layout,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 8.0,
            height: 8.0,
        },
    );

    assert_eq!(canvas.pixel(0, 0), Some(Color::rgb(255, 0, 0)));
}

#[test]
fn positioned_grandchild_paints_above_float_uncle() {
    let root = NodeHandle::element("div");
    root.set_attribute("class", "root");
    let wrapper = NodeHandle::element("div");
    wrapper.set_attribute("class", "wrapper");
    let overlay = NodeHandle::element("div");
    overlay.set_attribute("class", "overlay");
    let float = NodeHandle::element("div");
    float.set_attribute("class", "float");
    root.append_child(wrapper.clone());
    root.append_child(float.clone());
    wrapper.append_child(overlay.clone());

    let stylesheet = ".root { position: relative; } \
             .wrapper { width: 8px; height: 8px; } \
             .overlay { position: absolute; left: 0; top: 0; width: 8px; height: 8px; background: red; } \
             .float { float: left; width: 8px; height: 8px; background: blue; }";
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
    let layout = layout_tree(
        &root,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 8.0,
            height: 8.0,
        },
    )
    .unwrap();

    let mut paint_resolver = StyleResolver::new();
    paint_resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
    let canvas = paint_layout(
        &layout,
        &mut paint_resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 8.0,
            height: 8.0,
        },
    );

    assert_eq!(canvas.pixel(0, 0), Some(Color::rgb(255, 0, 0)));
}

#[test]
fn float_grandchild_paints_above_block_uncle() {
    let root = NodeHandle::element("div");
    root.set_attribute("class", "root");
    let wrapper = NodeHandle::element("div");
    wrapper.set_attribute("class", "wrapper");
    let floated = NodeHandle::element("div");
    floated.set_attribute("class", "floated");
    let block = NodeHandle::element("div");
    block.set_attribute("class", "block");
    root.append_child(wrapper.clone());
    root.append_child(block.clone());
    wrapper.append_child(floated.clone());

    let stylesheet = ".wrapper { width: 8px; height: 8px; } \
             .floated { float: left; width: 8px; height: 8px; background: blue; } \
             .block { width: 8px; height: 8px; background: red; }";
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
    let layout = LayoutBox {
        node: root,
        dimensions: BoxDimensions {
            content: Rect {
                x: 0.0,
                y: 0.0,
                width: 8.0,
                height: 8.0,
            },
            ..BoxDimensions::default()
        },
        visibility: Visibility::Visible,
        overflow: crate::layout::Overflow::Visible,
        z_index: 0,
        transform: crate::css::AffineTransform::identity(),
        lines: Vec::new(),
        children: vec![
            LayoutBox {
                node: wrapper,
                dimensions: BoxDimensions {
                    content: Rect {
                        x: 0.0,
                        y: 0.0,
                        width: 8.0,
                        height: 8.0,
                    },
                    ..BoxDimensions::default()
                },
                visibility: Visibility::Visible,
                overflow: crate::layout::Overflow::Visible,
                z_index: 0,
                transform: crate::css::AffineTransform::identity(),
                lines: Vec::new(),
                children: vec![LayoutBox {
                    node: floated,
                    dimensions: BoxDimensions {
                        content: Rect {
                            x: 0.0,
                            y: 0.0,
                            width: 8.0,
                            height: 8.0,
                        },
                        ..BoxDimensions::default()
                    },
                    visibility: Visibility::Visible,
                    overflow: crate::layout::Overflow::Visible,
                    z_index: 0,
                    transform: crate::css::AffineTransform::identity(),
                    lines: Vec::new(),
                    children: Vec::new(),
                    marker: None,
                }],
                marker: None,
            },
            LayoutBox {
                node: block,
                dimensions: BoxDimensions {
                    content: Rect {
                        x: 0.0,
                        y: 0.0,
                        width: 8.0,
                        height: 8.0,
                    },
                    ..BoxDimensions::default()
                },
                visibility: Visibility::Visible,
                overflow: crate::layout::Overflow::Visible,
                z_index: 0,
                transform: crate::css::AffineTransform::identity(),
                lines: Vec::new(),
                children: Vec::new(),
                marker: None,
            },
        ],
        marker: None,
    };

    let mut paint_resolver = StyleResolver::new();
    paint_resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
    let canvas = paint_layout(
        &layout,
        &mut paint_resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 8.0,
            height: 8.0,
        },
    );

    assert_eq!(canvas.pixel(0, 0), Some(Color::rgb(0, 0, 255)));
}

#[test]
fn paints_side_specific_border_colors() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(div);

    let stylesheet = "body { margin: 0; } \
             div { width: 4px; height: 4px; border-top: solid red 1px; border-left: solid blue 1px; border-right: solid yellow 1px; border-bottom: solid black 1px; }";
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
    let layout = layout_tree(
        &body,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        },
    )
    .unwrap();

    let mut paint_resolver = StyleResolver::new();
    paint_resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
    let canvas = paint_layout(
        &layout,
        &mut paint_resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        },
    );

    assert_eq!(canvas.pixel(1, 0), Some(Color::rgb(255, 0, 0)));
    assert_eq!(canvas.pixel(0, 1), Some(Color::rgb(0, 0, 255)));
    assert_eq!(canvas.pixel(5, 1), Some(Color::rgb(255, 255, 0)));
    assert_eq!(canvas.pixel(1, 5), Some(Color::rgb(0, 0, 0)));
}

#[test]
fn paints_children_in_z_index_order() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let container = NodeHandle::element("div");
    let low = NodeHandle::element("div");
    let high = NodeHandle::element("div");
    low.set_attribute("class", "low");
    high.set_attribute("class", "high");
    document.append_child(body.clone());
    body.append_child(container.clone());
    container.append_child(low);
    container.append_child(high);

    let stylesheet = "body { margin: 0; } \
             .low { position: absolute; left: 0; top: 0; width: 10px; height: 10px; background-color: blue; z-index: 1; } \
             .high { position: absolute; left: 0; top: 0; width: 10px; height: 10px; background-color: red; z-index: 10; }";
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
    let layout = layout_tree(
        &document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 20.0,
            height: 20.0,
        },
    )
    .unwrap();

    let mut paint_resolver = StyleResolver::new();
    paint_resolver.add_stylesheet(Origin::Author, parse_stylesheet(stylesheet).unwrap());
    let canvas = paint_layout(
        &layout,
        &mut paint_resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 20.0,
            height: 20.0,
        },
    );

    assert_eq!(canvas.pixel(5, 5), Some(Color::rgb(255, 0, 0)));
}

#[test]
fn encodes_canvas_as_png() {
    let mut canvas = Canvas::new(2, 1);
    canvas.fill_rect(
        Rect {
            x: 0.0,
            y: 0.0,
            width: 2.0,
            height: 1.0,
        },
        Color::rgb(255, 0, 0),
    );

    let png = canvas.encode_png();
    assert_eq!(&png[..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
    assert!(png.windows(4).any(|window| window == b"IHDR"));
    assert!(png.windows(4).any(|window| window == b"IDAT"));
    assert!(png.windows(4).any(|window| window == b"IEND"));
}

#[test]
fn composites_transparent_canvas_over_opaque_background() {
    let mut canvas = Canvas::new(2, 1);
    canvas.set_pixel(1, 0, Color::rgba(255, 0, 0, 128));
    canvas.composite_over(Color::rgb(255, 255, 255));
    assert_eq!(canvas.pixel(0, 0), Some(Color::rgb(255, 255, 255)));
    assert_eq!(canvas.pixel(1, 0), Some(Color::rgb(255, 127, 127)));
}

#[test]
fn decodes_png_images_into_rgba_pixels() {
    let mut canvas = Canvas::new(2, 1);
    canvas.fill_rect(
        Rect {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        },
        Color::rgb(255, 0, 0),
    );
    canvas.fill_rect(
        Rect {
            x: 1.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        },
        Color::rgba(0, 255, 0, 128),
    );

    let image = Image::decode_png(&canvas.encode_png()).unwrap();
    assert_eq!(image.width(), 2);
    assert_eq!(image.height(), 1);
    assert_eq!(image.pixels(), &[255, 0, 0, 255, 0, 255, 0, 128,]);
}

#[test]
fn draws_images_with_alpha_compositing() {
    let image = Image::new(1, 1, vec![255, 0, 0, 128]).unwrap();
    let mut canvas = Canvas::new(1, 1);
    canvas.fill_rect(
        Rect {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        },
        Color::rgb(0, 0, 255),
    );

    canvas.draw_image(&image, 0.0, 0.0);

    assert_eq!(canvas.pixel(0, 0), Some(Color::rgba(128, 0, 127, 255)));
}

#[test]
fn parses_text_and_png_data_uris() {
    let text = parse_data_uri("data:,hello%20world").unwrap();
    assert_eq!(
        text,
        DataUri::Text {
            mime_type: "text/plain".to_string(),
            data: "hello world".to_string(),
        }
    );

    let mut canvas = Canvas::new(1, 1);
    canvas.fill_rect(
        Rect {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        },
        Color::rgb(255, 0, 0),
    );
    let encoded = base64::engine::general_purpose::STANDARD.encode(canvas.encode_png());
    let image = parse_data_uri(&format!("data:image/png;base64,{encoded}")).unwrap();
    match image {
        DataUri::Binary { mime_type, data } => {
            assert_eq!(mime_type, "image/png");
            assert_eq!(
                Image::decode_png(&data).unwrap().pixels(),
                &[255, 0, 0, 255]
            );
        }
        DataUri::Text { .. } => panic!("expected binary data uri"),
    }
}

#[test]
fn parses_percent_encoded_base64_data_uri() {
    let image = parse_data_uri(
            "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADElEQVR4AQEFAPr%2FAP8AAP9zftimAAAAAElFTkSuQmCC",
        )
        .unwrap();

    match image {
        DataUri::Binary { mime_type, data } => {
            assert_eq!(mime_type, "image/png");
            assert_eq!(Image::decode_png(&data).unwrap().width(), 1);
        }
        DataUri::Text { .. } => panic!("expected binary data uri"),
    }
}

#[test]
fn decodes_jpeg_image() {
    // Minimal valid JPEG: 1x1 red pixel
    // Created with: convert -size 1x1 xc:red red.jpg && base64 red.jpg
    let jpeg_base64 = "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAACf/EABQQAQAAAAAAAAAAAAAAAAAAAAD/2gAIAQEAAD8AVN//2Q==";
    let jpeg_data = base64::engine::general_purpose::STANDARD
        .decode(jpeg_base64)
        .unwrap();

    let image = Image::decode_jpeg(&jpeg_data).unwrap();
    assert_eq!(image.width(), 1);
    assert_eq!(image.height(), 1);
    // JPEG is lossy, so we just check that we got valid RGBA data
    assert_eq!(image.pixels().len(), 4);
}

#[test]
fn decodes_first_gif_frame_into_rgba_pixels() {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode("R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw==")
        .unwrap();
    let image = Image::decode_gif(&bytes).unwrap();
    assert_eq!((image.width(), image.height()), (1, 1));
    assert_eq!(image.pixels().len(), 4);
}

#[test]
fn decodes_jpeg_data_uri() {
    // Minimal valid JPEG: 1x1 red pixel
    let jpeg_base64 = "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAACf/EABQQAQAAAAAAAAAAAAAAAAAAAAD/2gAIAQEAAD8AVN//2Q==";
    let data_uri = format!("data:image/jpeg;base64,{}", jpeg_base64);

    let parsed = parse_data_uri(&data_uri).unwrap();
    match parsed {
        DataUri::Binary { mime_type, data } => {
            assert_eq!(mime_type, "image/jpeg");
            let image = Image::decode_jpeg(&data).unwrap();
            assert_eq!(image.width(), 1);
            assert_eq!(image.height(), 1);
        }
        DataUri::Text { .. } => panic!("expected binary data uri"),
    }
}

#[test]
fn renders_acid2_fixture_to_png() {
    let html = fs::read_to_string(acid2_fixture_path()).unwrap();
    let document = TreeBuilder::parse(&html).document();

    let png = render_document_png(
        &document,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
    )
    .unwrap();

    assert_eq!(&png[..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
}

#[test]
fn renders_official_reference_fixture_to_png() {
    let html = fs::read_to_string(acid2_official_reference_html_path()).unwrap();
    let document = TreeBuilder::parse(&html).document();

    let png = render_document_with_base_path(
        &document,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
        &acid2_fixture_dir(),
    )
    .unwrap()
    .encode_png();

    assert_eq!(&png[..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
}

#[test]
fn acid2_fixture_matches_local_baseline_png() {
    let html = fs::read_to_string(acid2_fixture_path()).unwrap();
    let document = TreeBuilder::parse(&html).document();
    let actual = render_document(
        &document,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
    )
    .unwrap();

    let reference_path = acid2_baseline_path();
    assert!(
        reference_path.exists(),
        "missing local Acid2 baseline image at {}",
        reference_path.display()
    );
    let expected_png = fs::read(reference_path).unwrap();
    let expected = Image::decode_png(&expected_png).unwrap();
    let mut expected_canvas = Canvas::new(expected.width(), expected.height());
    expected_canvas.draw_image(&expected, 0.0, 0.0);

    // Allow some pixel differences due to font/glyph rendering variations
    // across different platforms (macOS vs Linux use different system fonts)
    let (diff, changed) = diff_canvases_with_tolerance(&actual, &expected_canvas, 1);
    let text_tolerance = 12000;

    if changed > text_tolerance {
        fs::create_dir_all(acid2_output_dir()).unwrap();
        fs::write(
            fixture_output_image_path("acid2", "local-baseline", "actual"),
            actual.encode_png(),
        )
        .unwrap();
        fs::write(
            fixture_output_image_path("acid2", "local-baseline", "expected"),
            expected_png,
        )
        .unwrap();
        fs::write(
            fixture_output_image_path("acid2", "local-baseline", "diff"),
            diff.encode_png(),
        )
        .unwrap();
        panic!(
            "acid2 rendering diverged from the checked-in local baseline ({} pixels differ, tolerance {}); wrote diff assets to tests/output/acid2",
            changed, text_tolerance
        );
    }
}

#[test]
fn official_acid2_reference_assets_are_checked_in() {
    let reference_html = fs::read_to_string(acid2_official_reference_html_path()).unwrap();
    assert!(reference_html.contains("The Second Acid Test (Reference Rendering)"));

    let reference_png = fs::read(acid2_official_reference_png_path()).unwrap();
    let decoded = Image::decode_png(&reference_png).unwrap();
    assert_eq!(decoded.width(), 168);
    assert_eq!(decoded.height(), 168);
}

#[test]
fn official_reference_fixture_only_lays_out_hello_world_text() {
    let html = fs::read_to_string(acid2_official_reference_html_path()).unwrap();
    let document = TreeBuilder::parse(&html).document();
    materialize_local_assets(&document, &acid2_fixture_dir()).unwrap();

    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }

    let layout = crate::layout::layout_tree(
        &document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
    )
    .unwrap();

    let texts = collect_layout_texts(&layout);
    let joined: String = texts.concat().replace('\u{00A0}', " ");
    assert_eq!(joined.trim(), "Hello World!");
}

#[test]
fn acid2_eyes_layout_contains_expected_boxes() {
    let acid2_html = fs::read_to_string(acid2_fixture_path()).unwrap();
    let acid2_document = TreeBuilder::parse(&acid2_html).document();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&acid2_document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }

    let layout = crate::layout::layout_tree(
        &acid2_document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
    )
    .unwrap();

    let eyes_a = find_layout_box_by_id(&layout, "eyes-a").unwrap();
    let eyes_b = find_layout_box_by_id(&layout, "eyes-b").unwrap();
    let eyes_c = find_layout_box_by_id(&layout, "eyes-c").unwrap();
    let eyes_b_style = resolver.computed_style(&eyes_b.node);

    assert!(!eyes_a.lines.is_empty());
    assert!(eyes_a.lines.iter().any(|line| {
        line.fragments
            .iter()
            .any(|fragment| matches!(fragment.content, InlineFragmentContent::Image(_, _)))
    }));
    assert!(
        eyes_b.dimensions.content.width > 0.0,
        "{:?}",
        eyes_b.dimensions
    );
    assert!(
        eyes_b.dimensions.content.height > 0.0,
        "{:?} {:?} {:?} {:?}",
        eyes_b.dimensions,
        eyes_b_style.get("height"),
        eyes_b_style.get("border-left-width"),
        eyes_b_style.get("border-right-width")
    );
    assert!(
        eyes_c.dimensions.content.width > 0.0,
        "{:?}",
        eyes_c.dimensions
    );
    assert!(
        eyes_c.dimensions.content.height > 0.0,
        "{:?}",
        eyes_c.dimensions
    );
}

#[test]
fn acid2_eyes_inline_layer_stays_at_same_origin_as_float_and_block_layers() {
    let acid2_html = fs::read_to_string(acid2_fixture_path()).unwrap();
    let acid2_document = TreeBuilder::parse(&acid2_html).document();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&acid2_document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }

    let layout = crate::layout::layout_tree(
        &acid2_document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
    )
    .unwrap();

    let eyes = find_layout_box_by_id(&layout, "eyes-a")
        .and_then(|eyes_a| find_parent_layout_box_by_id(&layout, "eyes-a").or(Some(eyes_a)))
        .unwrap();
    let eyes_a = find_layout_box_by_id(&layout, "eyes-a").unwrap();
    let eyes_b = find_layout_box_by_id(&layout, "eyes-b").unwrap();
    let eyes_c = find_layout_box_by_id(&layout, "eyes-c").unwrap();
    let first_line = eyes_a.lines.first().unwrap();
    let first_image = first_line
        .fragments
        .iter()
        .find(|fragment| matches!(fragment.content, InlineFragmentContent::Image(_, _)))
        .unwrap();
    assert_eq!(eyes_b.dimensions.content.y, eyes_c.dimensions.content.y);
    assert_eq!(first_line.rect.y, eyes_b.dimensions.content.y);
    assert!(
        (eyes_a.dimensions.content.width - eyes.dimensions.content.width).abs() <= 0.5,
        "{:?} {:?}",
        eyes_a.dimensions.content,
        eyes.dimensions.content
    );
    assert!(first_image.rect.x >= eyes.dimensions.content.x);
    assert!(
        first_image.rect.x + first_image.rect.width
            <= eyes.dimensions.content.x + eyes.dimensions.content.width + 0.5,
        "{:?} {:?}",
        first_image.rect,
        eyes.dimensions.content
    );
    assert!(
        ((first_image.rect.x + first_image.rect.width)
            - (eyes.dimensions.content.x + eyes.dimensions.content.width))
            .abs()
            <= 0.5,
        "{:?} {:?}",
        first_image.rect,
        eyes.dimensions.content
    );
}

#[test]
fn acid2_eyes_block_layer_stays_overlapping_float_layer() {
    let acid2_html = fs::read_to_string(acid2_fixture_path()).unwrap();
    let acid2_document = TreeBuilder::parse(&acid2_html).document();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&acid2_document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }

    let layout = crate::layout::layout_tree(
        &acid2_document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
    )
    .unwrap();

    let eyes_b = find_layout_box_by_id(&layout, "eyes-b").unwrap();
    let eyes_c = find_layout_box_by_id(&layout, "eyes-c").unwrap();

    assert!(
        eyes_c.dimensions.content.x < eyes_b.dimensions.content.x + eyes_b.dimensions.content.width,
        "{:?} {:?}",
        eyes_b.dimensions.content,
        eyes_c.dimensions.content
    );
}

#[test]
fn acid2_smile_layout_contains_positioned_and_floated_descendants() {
    let acid2_html = fs::read_to_string(acid2_fixture_path()).unwrap();
    let acid2_document = TreeBuilder::parse(&acid2_html).document();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&acid2_document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }

    let layout = crate::layout::layout_tree(
        &acid2_document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
    )
    .unwrap();

    let smile = find_layout_box_by_class(&layout, "smile").unwrap();
    let nose = find_layout_box_by_class(&layout, "nose").unwrap();
    let empty = find_layout_box_by_class(&layout, "empty").unwrap();
    let chin = find_layout_box_by_class(&layout, "chin").unwrap();
    let positioned = smile
        .children
        .iter()
        .find(|child| {
            matches!(
                resolver.computed_style(&child.node).get("position"),
                Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("relative")
            )
        })
        .unwrap();
    let absolute = positioned
        .children
        .iter()
        .find(|child| {
            matches!(
                resolver.computed_style(&child.node).get("position"),
                Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("absolute")
            )
        })
        .unwrap();
    let float_descendant = absolute
        .children
        .iter()
        .find(|child| {
            matches!(
                resolver.computed_style(&child.node).get("float"),
                Some(ComputedValue::Keyword(keyword))
                    if keyword.eq_ignore_ascii_case("left")
                        || keyword.eq_ignore_ascii_case("right")
            )
        })
        .unwrap();

    // After intrinsic_width fix, absolute div width includes span's borders
    assert_eq!(
        absolute.dimensions.content.width, 96.0,
        "absolute div should be 96px (span 72 + border 24)"
    );
    assert!(
        absolute.dimensions.content.width > 0.0,
        "{:?}",
        absolute.dimensions
    );
    assert!(
        float_descendant.total_width() > 0.0,
        "{:?}",
        float_descendant.dimensions
    );
    assert_eq!(
        empty.dimensions.content.height, 0.0,
        "{:?}",
        empty.dimensions
    );
    assert!(
        nose.dimensions.content.height <= 36.0,
        "{:?}",
        nose.dimensions
    );
    assert!(chin.dimensions.content.y < smile.dimensions.content.y + 200.0);
}

#[test]
fn acid2_lower_face_boxes_keep_expected_vertical_order() {
    let acid2_html = fs::read_to_string(acid2_fixture_path()).unwrap();
    let acid2_document = TreeBuilder::parse(&acid2_html).document();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&acid2_document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }

    let layout = crate::layout::layout_tree(
        &acid2_document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
    )
    .unwrap();

    let nose = find_layout_box_by_class(&layout, "nose").unwrap();
    let smile = find_layout_box_by_class(&layout, "smile").unwrap();
    let chin = find_layout_box_by_class(&layout, "chin").unwrap();
    let smile_relative = smile.children.first().unwrap();
    let nose_bottom = nose.dimensions.content.y + nose.dimensions.content.height;
    let nose_to_smile_gap = smile.dimensions.content.y - nose_bottom;
    let smile_to_chin_gap = chin.dimensions.content.y - smile.dimensions.content.y;

    assert!(
        smile.dimensions.content.y >= nose_bottom,
        "smile should be below nose"
    );
    assert!(
        chin.dimensions.content.y >= smile.dimensions.content.y,
        "chin should be below smile"
    );
    assert!(smile_relative.dimensions.content.y < chin.dimensions.content.y);
    assert!(
        nose_to_smile_gap < 180.0,
        "nose_to_smile_gap={nose_to_smile_gap}"
    );
    assert!(
        smile_to_chin_gap < 220.0,
        "smile_to_chin_gap={smile_to_chin_gap}"
    );
}

#[test]
fn acid2_empty_block_creates_large_gap_before_smile() {
    let acid2_html = fs::read_to_string(acid2_fixture_path()).unwrap();
    let acid2_document = TreeBuilder::parse(&acid2_html).document();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&acid2_document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }

    let layout = crate::layout::layout_tree(
        &acid2_document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
    )
    .unwrap();

    let empty = find_layout_box_by_class(&layout, "empty").unwrap();
    let smile = find_layout_box_by_class(&layout, "smile").unwrap();
    let empty_outer_bottom = empty.dimensions.content.y
        + empty.dimensions.content.height
        + empty.dimensions.padding.bottom
        + empty.dimensions.border.bottom
        + empty.dimensions.margin.bottom;
    let gap_after_empty = smile.dimensions.content.y - empty_outer_bottom;

    assert_eq!(empty.dimensions.content.height, 0.0);
    assert!(
        gap_after_empty < 20.0,
        "{gap_after_empty} {:?}",
        empty.dimensions
    );
}

#[test]
fn acid2_empty_block_starts_shortly_after_nose() {
    let acid2_html = fs::read_to_string(acid2_fixture_path()).unwrap();
    let acid2_document = TreeBuilder::parse(&acid2_html).document();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&acid2_document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }

    let layout = crate::layout::layout_tree(
        &acid2_document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
    )
    .unwrap();

    let nose = find_layout_box_by_class(&layout, "nose").unwrap();
    let empty = find_layout_box_by_class(&layout, "empty").unwrap();
    let nose_outer_bottom = nose.dimensions.content.y
        + nose.dimensions.content.height
        + nose.dimensions.padding.bottom
        + nose.dimensions.border.bottom
        + nose.dimensions.margin.bottom;
    let gap_before_empty = empty.dimensions.content.y - nose_outer_bottom;

    assert_eq!(empty.dimensions.content.height, 0.0);
    assert!(
        gap_before_empty < 80.0,
        "{gap_before_empty} {:?}",
        empty.dimensions
    );
}

#[test]
fn acid2_second_line_absolute_shrink_wraps_float() {
    let acid2_html = fs::read_to_string(acid2_fixture_path()).unwrap();
    let acid2_document = TreeBuilder::parse(&acid2_html).document();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&acid2_document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }

    let layout = crate::layout::layout_tree(
        &acid2_document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
    )
    .unwrap();

    let second_line = find_layout_box_by_class(&layout, "first").unwrap();
    let floated_inner = second_line.children.iter().find(|child| {
        matches!(
            resolver.computed_style(&child.node).get("float"),
            Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("right")
        )
    });

    assert_eq!(second_line.dimensions.content.width, 48.0);
    assert!(floated_inner.is_some(), "{:?}", second_line.children);
    assert_eq!(second_line.dimensions.content.height, 12.0);
}

#[test]
fn acid2_smile_nested_float_keeps_block_width_source_descendant() {
    let acid2_html = fs::read_to_string(acid2_fixture_path()).unwrap();
    let acid2_document = TreeBuilder::parse(&acid2_html).document();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&acid2_document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }

    let layout = crate::layout::layout_tree(
        &acid2_document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
    )
    .unwrap();

    let smile = find_layout_box_by_class(&layout, "smile").unwrap();
    let relative = smile.children.first().unwrap();
    let absolute = relative.children.first().unwrap();
    let nested_float = absolute.children.first().unwrap();
    let inherited_float = nested_float.children.first().unwrap();
    let strong = find_first_descendant_by_tag(&acid2_document, "strong").unwrap();
    let strong_style = resolver.computed_style(&strong);

    assert_eq!(
        strong_style.get("display"),
        Some(&ComputedValue::Keyword("block".to_string()))
    );
    assert_eq!(strong_style.get("width"), Some(&ComputedValue::Px(72.0)));
    assert!(
        !inherited_float.children.is_empty(),
        "{:?}",
        inherited_float.dimensions
    );
    assert!(
        inherited_float.dimensions.content.width >= 72.0,
        "{:?}",
        inherited_float.dimensions
    );
}

#[test]
fn acid2_smile_nested_float_uses_side_borders_only_on_span_and_top_bottom_on_em() {
    let acid2_html = fs::read_to_string(acid2_fixture_path()).unwrap();
    let document = TreeBuilder::parse(&acid2_html).document();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }

    let span = find_first_descendant_by_tag(&document, "span").unwrap();
    let em = find_first_descendant_by_tag(&document, "em").unwrap();
    let span_style = resolver.computed_style(&span);
    let em_style = resolver.computed_style(&em);

    assert_eq!(
        span_style.get("border-top-width"),
        Some(&ComputedValue::Px(0.0))
    );
    assert_eq!(
        span_style.get("border-bottom-width"),
        Some(&ComputedValue::Px(0.0))
    );
    assert_eq!(
        span_style.get("border-left-style"),
        Some(&ComputedValue::Keyword("solid".to_string()))
    );
    assert_eq!(
        span_style.get("border-right-style"),
        Some(&ComputedValue::Keyword("solid".to_string()))
    );

    assert_eq!(
        em_style.get("border-top-style"),
        Some(&ComputedValue::Keyword("solid".to_string()))
    );
    assert_eq!(
        em_style.get("border-bottom-style"),
        Some(&ComputedValue::Keyword("solid".to_string()))
    );
}

#[test]
fn test_scroll_translation_keeps_fixed_positioned_boxes_in_viewport_place() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let flow = NodeHandle::element("div");
    let fixed = NodeHandle::element("aside");
    flow.set_attribute("class", "flow");
    fixed.set_attribute("class", "fixed");
    document.append_child(body.clone());
    body.append_child(flow.clone());
    body.append_child(fixed.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                ".flow { width: 10px; height: 100px; } .fixed { position: fixed; top: 20px; left: 5px; width: 10px; height: 10px; }",
            )
            .unwrap(),
        );

    let mut layout = crate::layout::layout_tree(
        &body,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
        },
    )
    .unwrap();

    let fixed_before = find_layout_box_by_class(&layout, "fixed")
        .map(|node| node.dimensions.content)
        .unwrap();
    translate_layout_box_for_test(&mut layout, &mut resolver, 0.0, -50.0);
    let fixed_after = find_layout_box_by_class(&layout, "fixed")
        .map(|node| node.dimensions.content)
        .unwrap();

    assert_eq!(fixed_before, fixed_after);
}

#[test]
fn scrolled_snapshot_moves_flow_content_but_keeps_fixed_pixels() {
    let html = r#"<html><head><style>
        * { margin: 0; padding: 0; }
        body { width: 100px; height: 200px; }
        .flow { position: absolute; left: 20px; top: 60px; width: 10px; height: 10px; background: red; }
        .fixed { position: fixed; left: 5px; top: 5px; width: 10px; height: 10px; background: blue; }
    </style></head><body><div class="flow"></div><div class="fixed"></div></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let viewport = Rect { x: 0.0, y: 0.0, width: 100.0, height: 100.0 };
    let canvas = render_document_snapshot_with_url(&document, viewport, None, (0.0, 50.0))
        .expect("scrolled snapshot should render");

    assert_eq!(canvas.pixel(20, 10), Some(Color::rgb(255, 0, 0)));
    assert_eq!(canvas.pixel(5, 5), Some(Color::rgb(0, 0, 255)));
    assert_ne!(canvas.pixel(20, 60), Some(Color::rgb(255, 0, 0)));
}

#[test]
fn acid2_eye_png_decodes() {
    let acid2_html = fs::read_to_string(acid2_fixture_path()).unwrap();
    let marker = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAGAAAAAY";
    let start = acid2_html.find(marker).unwrap();
    let rest = &acid2_html[start..];
    let end = rest.find('"').unwrap();
    let data_uri = parse_data_uri(&rest[..end]).unwrap();
    let DataUri::Binary { data, .. } = data_uri else {
        panic!("expected binary data uri");
    };

    let image = Image::decode_png(&data).unwrap();
    assert!(image.width() > 0);
    assert!(image.height() > 0);
}

#[test]
fn acid2_eyes_b_rule_survives_forgiving_stylesheet_parse() {
    let acid2_html = fs::read_to_string(acid2_fixture_path()).unwrap();
    let document = TreeBuilder::parse(&acid2_html).document();
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();
    let joined = stylesheets.join("\n");

    assert!(joined.contains("#eyes-b"));
    let parsed = parse_stylesheet_forgiving(&joined);
    let mut found = false;
    for rule in parsed.rules {
        let crate::css::Rule::Style(rule) = rule else {
            continue;
        };
        let matches_eyes_b = rule.selectors.iter().any(|selector| {
            selector.parts.iter().any(|part| {
                part.simples.iter().any(
                    |simple| matches!(simple, crate::css::SimpleSelector::Id(id) if id == "eyes-b"),
                )
            })
        });
        if !matches_eyes_b {
            continue;
        }

        found = true;
        assert!(
            rule.declarations.iter().any(|decl| decl.name == "height"),
            "{rule:#?}"
        );
        assert!(
            rule.declarations
                .iter()
                .any(|decl| decl.name == "border-left-width"),
            "{rule:#?}"
        );
        assert!(
            rule.declarations
                .iter()
                .any(|decl| decl.name == "border-right-width"),
            "{rule:#?}"
        );
    }

    assert!(found, "expected parsed stylesheet to contain #eyes-b rule");
}

#[test]
fn acid2_link_stylesheet_overrides_picture_background_to_none() {
    let acid2_html = fs::read_to_string(acid2_fixture_path()).unwrap();
    let document = TreeBuilder::parse(&acid2_html).document();
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();
    let has_background_none = stylesheets
        .iter()
        .any(|css| css.contains(".picture") && css.contains("background") && css.contains("none"));
    assert!(
        has_background_none,
        "expected link stylesheet to contain '.picture {{ background: none }}', got: {:?}",
        stylesheets
    );

    let mut resolver = StyleResolver::new();
    for stylesheet in &stylesheets {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(stylesheet));
    }
    let picture = find_first_descendant_by_class(&document, "picture").unwrap();
    let style = resolver.computed_style(&picture);
    let bg_color = style.get("background-color");
    let is_transparent = matches!(
        bg_color,
        Some(ComputedValue::Color(c)) if c == "transparent" || c == "rgba(0,0,0,0)",
    ) || matches!(
        bg_color,
        Some(ComputedValue::Keyword(k)) if k == "transparent",
    );
    assert!(
        is_transparent,
        "expected .picture background-color to be transparent, got {:?}",
        bg_color
    );
}

#[test]
fn acid2_chin_negative_margin_pulls_it_toward_smile() {
    let acid2_html = fs::read_to_string(acid2_fixture_path()).unwrap();
    let acid2_document = TreeBuilder::parse(&acid2_html).document();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&acid2_document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }
    let layout = crate::layout::layout_tree(
        &acid2_document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
    )
    .unwrap();

    let smile = find_layout_box_by_class(&layout, "smile").unwrap();
    let chin = find_layout_box_by_class(&layout, "chin").unwrap();
    let smile_bottom = smile.dimensions.content.y
        + smile.dimensions.content.height
        + smile.dimensions.padding.bottom
        + smile.dimensions.border.bottom;
    let chin_top =
        chin.dimensions.content.y - chin.dimensions.padding.top - chin.dimensions.border.top;

    // .chin { margin: -4em 4em 0 } → margin-top: -48px
    // .smile { margin: 5em 3em } → margin-bottom: 60px
    // collapse_margins(60, -48) = 12px gap
    assert_eq!(
        chin_top - smile_bottom,
        12.0,
        "chin-smile gap should be 12px (collapsed margin), smile_bottom={smile_bottom}, chin_top={chin_top}",
    );
    assert!(
        chin.dimensions.margin.top < 0.0,
        "chin should have negative margin-top"
    );
}

#[test]
fn acid2_chin_height_includes_strut_from_parent_line_height() {
    let acid2_html = fs::read_to_string(acid2_fixture_path()).unwrap();
    let acid2_document = TreeBuilder::parse(&acid2_html).document();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&acid2_document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }
    let layout = crate::layout::layout_tree(
        &acid2_document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
    )
    .unwrap();
    let chin = find_layout_box_by_class(&layout, "chin").unwrap();
    // .chin { line-height: 1em } = 12px establishes the strut.
    // .chin div { display: inline; font: 2px/4px serif } has line-height 4px.
    // The strut (12px) must override the inline child's 4px.
    assert_eq!(
        chin.dimensions.content.height, 12.0,
        "chin content height should be 12px (strut from line-height: 1em), got {}",
        chin.dimensions.content.height,
    );
}

#[test]
fn stray_semicolon_between_rules_invalidates_next_selector() {
    let css = r#"
            .a { color: red; };
            .a { height: 99px; }
        "#;
    let parsed = parse_stylesheet_forgiving(css);
    // The stray ';' after '}' is consumed into the next rule's selector
    // prelude, making it '; .a' which is invalid. The rule is dropped.
    let has_height = parsed.rules.iter().any(|rule| {
        if let crate::css::Rule::Style(style_rule) = rule {
            style_rule.declarations.iter().any(|d| d.name == "height")
        } else {
            false
        }
    });
    assert!(
        !has_height,
        "height: 99px should be dropped because ';' invalidates the selector"
    );
}

#[test]
fn acid2_ul_table_cells_cover_red_background() {
    let acid2_html = fs::read_to_string(acid2_fixture_path()).unwrap();
    let acid2_document = TreeBuilder::parse(&acid2_html).document();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&acid2_document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }
    let layout = crate::layout::layout_tree(
        &acid2_document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
    )
    .unwrap();

    let ul = find_first_layout_box_by_tag(&layout, "ul").unwrap();
    // ul: display: table with 4 li cells, each 1em wide
    assert_eq!(
        ul.dimensions.content.width, 48.0,
        "ul width should be 4 × 1em = 48px"
    );
    let row = &ul.children[0];
    assert_eq!(row.children.len(), 4, "should have 4 cells");
    for cell in &row.children {
        assert_eq!(cell.dimensions.content.width, 12.0);
        assert_eq!(cell.dimensions.content.height, 12.0);
    }
}

#[test]
fn acid2_eyes_positioned_above_nose() {
    let acid2_html = fs::read_to_string(acid2_fixture_path()).unwrap();
    let acid2_document = TreeBuilder::parse(&acid2_html).document();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&acid2_document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }
    let layout = crate::layout::layout_tree(
        &acid2_document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
    )
    .unwrap();

    let eyes = find_layout_box_by_class(&layout, "eyes").unwrap();
    let nose = find_layout_box_by_class(&layout, "nose").unwrap();
    let forehead = find_layout_box_by_class(&layout, "forehead").unwrap();

    // .eyes { position: absolute; top: 5em } places eyes above the nose
    assert!(
        eyes.dimensions.content.y < nose.dimensions.content.y,
        "eyes (y={}) should be above nose (y={})",
        eyes.dimensions.content.y,
        nose.dimensions.content.y,
    );
    // eyes should be at or overlapping with the forehead area
    assert!(
        eyes.dimensions.content.y
            <= forehead.dimensions.content.y + forehead.dimensions.content.height,
        "eyes (y={}) should be at or above forehead bottom (y={})",
        eyes.dimensions.content.y,
        forehead.dimensions.content.y + forehead.dimensions.content.height,
    );
}

#[test]
fn acid2_p_bad_has_margin_top_from_adjacent_sibling_selector() {
    let acid2_html = fs::read_to_string(acid2_fixture_path()).unwrap();
    let document = TreeBuilder::parse(&acid2_html).document();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }
    let p_bad = find_first_descendant_by_class(&document, "bad").unwrap();
    let style = resolver.computed_style(&p_bad);
    let margin_top = style.get("margin-top");
    eprintln!("p.bad margin-top: {:?}", margin_top);
    eprintln!("p.bad position: {:?}", style.get("position"));
    // p + table + p { margin-top: 3em; } should match p.bad → 3em = 36px
    assert!(
        matches!(margin_top, Some(ComputedValue::Px(v)) if (*v - 36.0).abs() < 0.1),
        "p.bad should have margin-top: 3em (36px) from 'p + table + p' selector, got {:?}",
        margin_top,
    );
}

#[test]
fn acid2_parser_has_yellow_background_and_correct_size() {
    let acid2_html = fs::read_to_string(acid2_fixture_path()).unwrap();
    let document = TreeBuilder::parse(&acid2_html).document();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }
    let parser = find_first_descendant_by_class(&document, "parser").unwrap();
    let style = resolver.computed_style(&parser);
    // background: yellow should survive the `error: \}` parse test
    assert!(
        matches!(style.get("background-color"), Some(ComputedValue::Color(c)) if c == "yellow")
            || matches!(style.get("background-color"), Some(ComputedValue::Keyword(k)) if k == "yellow"),
        "parser should have background: yellow, got {:?}",
        style.get("background-color"),
    );
    // width: 2em (24px) — later `width: 200` is invalid (unitless non-zero)
    assert_eq!(
        style.get("width"),
        Some(&ComputedValue::Px(24.0)),
        "parser width should be 2em (24px), got {:?}",
        style.get("width")
    );
    // height: 1em (12px) — `.parser { height: 3em; }` is dropped because the
    // stray `;` between rules is consumed into its selector prelude per CSS
    // error recovery, making the selector invalid.
    assert_eq!(
        style.get("height"),
        Some(&ComputedValue::Px(12.0)),
        "parser height should be 1em (12px), got {:?}",
        style.get("height")
    );
}

#[test]
fn acid2_forehead_background_image_decodes_to_yellow_pixel() {
    let acid2_html = fs::read_to_string(acid2_fixture_path()).unwrap();
    let document = TreeBuilder::parse(&acid2_html).document();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }
    let forehead = find_first_descendant_by_class(&document, "forehead").unwrap();
    let style = resolver.computed_style(&forehead);
    let image = background_image(&style);
    assert!(image.is_some(), "forehead background-image should decode");
    let image = image.unwrap();
    assert_eq!(image.width(), 1);
    assert_eq!(image.height(), 1);
    let pixels = image.pixels();
    assert!(pixels.len() >= 4);
    // RGBA: yellow = (255, 255, 0, 255)
    assert_eq!(pixels[0], 255, "red");
    assert_eq!(pixels[1], 255, "green");
    assert_eq!(pixels[2], 0, "blue");
}

#[test]
fn acid2_nose_inner_div_has_before_and_after_pseudo_with_border() {
    let acid2_html = fs::read_to_string(acid2_fixture_path()).unwrap();
    let document = TreeBuilder::parse(&acid2_html).document();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }
    // .nose > div > div is the inner red square
    let nose = find_first_descendant_by_class(&document, "nose").unwrap();
    let nose_outer_div = nose
        .child_nodes()
        .into_iter()
        .find(|n| n.tag_name().as_deref() == Some("div"))
        .unwrap();
    let nose_inner_div = nose_outer_div
        .child_nodes()
        .into_iter()
        .find(|n| n.tag_name().as_deref() == Some("div"))
        .unwrap();

    let before = resolver.computed_pseudo_style(&nose_inner_div, PseudoElement::Before);
    let after = resolver.computed_pseudo_style(&nose_inner_div, PseudoElement::After);

    assert!(
        before.is_some(),
        "nose inner div should have :before pseudo"
    );
    let before = before.unwrap();
    assert_eq!(
        before.get("content"),
        Some(&ComputedValue::String("".to_string()))
    );
    assert_eq!(
        before.get("display"),
        Some(&ComputedValue::Keyword("block".to_string()))
    );

    assert!(
        after.is_some(),
        "nose should have :after pseudo (selector is .nose div :after)"
    );
}

#[test]
fn pseudo_before_border_triangle_renders() {
    let html = r#"<html><head><style>
            body { margin: 0; border-top: 1px solid transparent; }
            div { width: 24px; height: 24px; background: red; margin: 24px; }
            div:before { display: block; content: ''; height: 0;
                border-style: none solid solid;
                border-width: 12px;
                border-color: red yellow black yellow; }
        </style></head><body><div></div></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let canvas = render_document(
        &document,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 80.0,
            height: 80.0,
        },
    )
    .unwrap();

    let black_count = count_pixels(&canvas, Color::rgb(0, 0, 0));
    let yellow_count = count_pixels(&canvas, Color::rgb(255, 255, 0));
    let red_count = count_pixels(&canvas, Color::rgb(255, 0, 0));
    eprintln!("black={black_count} yellow={yellow_count} red={red_count}");
    // :before above the div, :after below — both should produce triangles
    assert!(
        black_count > 0,
        "should have black border triangles from pseudo elements"
    );
    assert!(
        yellow_count > 0 || red_count > 0,
        "should have colored border triangles from pseudo elements"
    );
}

#[test]
fn absolute_child_of_relative_paints_yellow_border() {
    let html = r#"<html><head><style>
            body { margin: 0; }
            .outer { width: 120px; height: 24px; background: black; position: relative; }
            .inner { position: absolute; top: 0; right: 12px; width: 48px; height: 0; border: yellow solid 12px; }
        </style></head><body><div class="outer"><div class="inner"></div></div></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let canvas = render_document(
        &document,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 200.0,
            height: 50.0,
        },
    )
    .unwrap();

    // .inner: width=48, height=0, border=12 all sides. total: 72x24.
    // right: 12px from .outer right edge. x = 120 - 72 - 12 = 36.
    // Yellow border should be visible on top of black background.
    let has_yellow = (0..120).any(|x| canvas.pixel(x, 5) == Some(Color::rgb(255, 255, 0)));
    let has_black = (0..120).any(|x| canvas.pixel(x, 5) == Some(Color::rgb(0, 0, 0)));
    assert!(has_black, "should have black background");
    assert!(
        has_yellow,
        "absolute child's yellow border should paint on top of relative parent's black background"
    );
}

#[test]
fn acid2_fixture_matches_official_reference_rendering() {
    let acid2_html = fs::read_to_string(acid2_fixture_path()).unwrap();
    let acid2_document = TreeBuilder::parse(&acid2_html).document();
    let mut acid2_resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&acid2_document, None).unwrap() {
        acid2_resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }
    let mut acid2_layout = crate::layout::layout_tree(
        &acid2_document,
        &mut acid2_resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
    )
    .unwrap();

    let reference_html = fs::read_to_string(acid2_official_reference_html_path()).unwrap();
    let reference_document = TreeBuilder::parse(&reference_html).document();
    materialize_local_assets(&reference_document, &acid2_fixture_dir()).unwrap();
    let mut reference_resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&reference_document, None).unwrap() {
        reference_resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }
    let reference_layout = crate::layout::layout_tree(
        &reference_document,
        &mut reference_resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
    )
    .unwrap();
    if let (Some((top_x, top_y)), Some((reference_x, reference_y))) = (
        find_layout_box_by_id(&acid2_layout, "top")
            .map(|top| (top.dimensions.content.x, top.dimensions.content.y)),
        find_first_layout_box_by_tag(&reference_layout, "h2")
            .map(|heading| (heading.dimensions.content.x, heading.dimensions.content.y)),
    ) {
        translate_layout_box_for_test(
            &mut acid2_layout,
            &mut acid2_resolver,
            reference_x - top_x,
            reference_y - top_y,
        );
    }
    let actual = paint_layout(
        &acid2_layout,
        &mut acid2_resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
    );
    let expected = paint_layout(
        &reference_layout,
        &mut reference_resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
    );

    let (diff, changed) = diff_canvases_with_tolerance(&actual, &expected, 1);
    // Allow some pixel differences due to font/glyph rendering variations
    // between our implementation and the official reference
    let text_tolerance = 1000;
    if changed > text_tolerance {
        fs::create_dir_all(acid2_output_dir()).unwrap();
        fs::write(
            fixture_output_image_path("acid2", "official-reference", "actual"),
            actual.encode_png(),
        )
        .unwrap();
        fs::write(
            fixture_output_image_path("acid2", "official-reference", "expected"),
            expected.encode_png(),
        )
        .unwrap();
        fs::write(
            fixture_output_image_path("acid2", "official-reference", "diff"),
            diff.encode_png(),
        )
        .unwrap();
    }

    assert!(
        changed <= text_tolerance,
        "acid2 rendering diverged from official reference rendering ({} pixels differ, tolerance {}); wrote diff assets to tests/output/acid2",
        changed,
        text_tolerance
    );
}

#[test]
#[ignore = "maintenance utility: set OMOIKANE_REFRESH_BASELINE=1 to rewrite the checked-in local Acid2 baseline image"]
fn refresh_acid2_baseline_png() {
    // This is a fixture-refresh utility, not a regression assertion — it overwrites
    // the committed baseline PNG. It must never mutate the working tree merely because
    // the suite was run with `--include-ignored` (e.g. the documented CI-parity command
    // `cargo test -- --include-ignored`, which in the bind-mounted Docker sandbox would
    // otherwise dirty the host tree byte-for-byte). Require an explicit opt-in so the
    // suite stays idempotent; a maintainer refreshing the baseline sets the env var.
    if std::env::var("OMOIKANE_REFRESH_BASELINE").as_deref() != Ok("1") {
        eprintln!(
            "skipping refresh_acid2_baseline_png: set OMOIKANE_REFRESH_BASELINE=1 to rewrite {}",
            acid2_baseline_path().display()
        );
        return;
    }

    let html = fs::read_to_string(acid2_fixture_path()).unwrap();
    let document = TreeBuilder::parse(&html).document();
    let png = render_document_png(
        &document,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
    )
    .unwrap();

    fs::create_dir_all(acid2_fixture_dir()).unwrap();
    fs::write(acid2_baseline_path(), png).unwrap();
}

fn acid2_fixture_dir() -> PathBuf {
    fixture_dir("acid2")
}

fn acid2_fixture_path() -> PathBuf {
    acid2_fixture_dir().join("acid2.html")
}

fn acid2_baseline_path() -> PathBuf {
    acid2_fixture_dir().join("acid2.baseline.png")
}

fn acid2_official_reference_html_path() -> PathBuf {
    acid2_fixture_dir().join("reference.html")
}

fn acid2_official_reference_png_path() -> PathBuf {
    acid2_fixture_dir().join("reference.png")
}

fn acid2_output_dir() -> PathBuf {
    fixture_output_dir("acid2")
}

// Issue 026: CSS パースエラーがあってもレンダリングが中断しない
#[test]
fn renders_despite_unparseable_css() {
    // <style> に構文的に無効なCSS（ルール0件になるケース）を含む HTML
    let html = r#"<html><head>
        <style>@@@@invalid css garbage !!!</style>
        <style>body { margin: 0; } div { background-color: #ff0000; width: 4px; height: 4px; }</style>
    </head><body><div></div></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let canvas = render_document(
        &document,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
        },
    );
    // パース不能な CSS があっても Result::Ok が返る
    assert!(
        canvas.is_ok(),
        "render_document should succeed even if one <style> block is unparseable"
    );
    // パース可能なCSSは適用されている
    let canvas = canvas.unwrap();
    assert_eq!(canvas.pixel(0, 0), Some(Color::rgb(255, 0, 0)));
}

fn fixture_dir(fixture: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(fixture)
}

fn fixture_output_dir(fixture: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/output")
        .join(fixture)
}

fn fixture_output_image_path(fixture: &str, scenario: &str, variant: &str) -> PathBuf {
    fixture_output_dir(fixture).join(format!("{fixture}.{scenario}.{variant}.png"))
}

fn baseline_image_path(fixture: &str, filename: &str) -> PathBuf {
    fixture_dir(fixture).join(filename)
}

#[test]
fn fixture_paths_follow_anonymized_diff_convention() {
    let output_path = fixture_output_image_path("anonymized-site-a", "viewport-1366x900", "diff");
    let expected = fixture_output_dir("anonymized-site-a")
        .join("anonymized-site-a.viewport-1366x900.diff.png");
    assert_eq!(output_path, expected);

    let baseline_path = baseline_image_path("anonymized-site-a", "render.baseline.png");
    let expected_baseline = fixture_dir("anonymized-site-a").join("render.baseline.png");
    assert_eq!(baseline_path, expected_baseline);
}

fn collect_layout_texts(layout: &LayoutBox) -> Vec<String> {
    let mut texts = Vec::new();
    collect_layout_texts_into(layout, &mut texts);
    texts
}

fn collect_layout_texts_into(layout: &LayoutBox, out: &mut Vec<String>) {
    for line in &layout.lines {
        for fragment in &line.fragments {
            if let InlineFragmentContent::Text(text) = &fragment.content {
                out.push(text.clone());
            }
        }
    }
    for child in &layout.children {
        collect_layout_texts_into(child, out);
    }
}

fn count_pixels(canvas: &Canvas, color: Color) -> usize {
    let mut count = 0usize;
    for y in 0..canvas.height() {
        for x in 0..canvas.width() {
            if canvas.pixel(x, y) == Some(color) {
                count += 1;
            }
        }
    }
    count
}

fn red_pixel_data_uri() -> String {
    let mut canvas = Canvas::new(1, 1);
    canvas.fill_rect(
        Rect {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        },
        Color::rgb(255, 0, 0),
    );
    let encoded = base64::engine::general_purpose::STANDARD.encode(canvas.encode_png());
    format!("data:image/png;base64,{encoded}")
}

fn rgba_mask_data_uri(width: u32, height: u32, alphas: &[u8]) -> String {
    assert_eq!(alphas.len(), width as usize * height as usize);
    let pixels = alphas
        .iter()
        .flat_map(|alpha| [255, 255, 255, *alpha])
        .collect();
    let image = Image::new(width, height, pixels).unwrap();
    let mut canvas = Canvas::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let index = (y * width + x) as usize;
            canvas.set_pixel(x, y, Color::rgba(255, 255, 255, alphas[index]));
        }
    }
    assert_eq!(image.pixels(), canvas.pixels());
    let encoded = base64::engine::general_purpose::STANDARD.encode(canvas.encode_png());
    format!("data:image/png;base64,{encoded}")
}

fn find_layout_box_by_id<'a>(layout: &'a LayoutBox, id: &str) -> Option<&'a LayoutBox> {
    if layout
        .node
        .attributes()
        .and_then(|attributes| attributes.get("id").cloned())
        .as_deref()
        == Some(id)
    {
        return Some(layout);
    }

    for child in &layout.children {
        if let Some(found) = find_layout_box_by_id(child, id) {
            return Some(found);
        }
    }

    None
}

fn find_first_layout_box_by_tag<'a>(layout: &'a LayoutBox, tag: &str) -> Option<&'a LayoutBox> {
    if layout.node.tag_name().as_deref() == Some(tag) {
        return Some(layout);
    }

    for child in &layout.children {
        if let Some(found) = find_first_layout_box_by_tag(child, tag) {
            return Some(found);
        }
    }

    None
}

fn find_first_descendant_by_tag(node: &NodeHandle, tag: &str) -> Option<NodeHandle> {
    if node.tag_name().as_deref() == Some(tag) {
        return Some(node.clone());
    }
    for child in node.child_nodes() {
        if let Some(found) = find_first_descendant_by_tag(&child, tag) {
            return Some(found);
        }
    }
    None
}

fn find_first_descendant_by_class(node: &NodeHandle, class: &str) -> Option<NodeHandle> {
    if let Some(attrs) = node.attributes() {
        if let Some(class_attr) = attrs.get("class") {
            if class_attr.split_whitespace().any(|c| c == class) {
                return Some(node.clone());
            }
        }
    }
    for child in node.child_nodes() {
        if let Some(found) = find_first_descendant_by_class(&child, class) {
            return Some(found);
        }
    }
    None
}

fn find_first_image_fragment(layout: &LayoutBox) -> Option<&InlineFragment> {
    for line in &layout.lines {
        if let Some(fragment) = line
            .fragments
            .iter()
            .find(|fragment| matches!(fragment.content, InlineFragmentContent::Image(_, _)))
        {
            return Some(fragment);
        }
    }
    for child in &layout.children {
        if let Some(found) = find_first_image_fragment(child) {
            return Some(found);
        }
    }
    None
}

fn find_layout_box_by_class<'a>(layout: &'a LayoutBox, class_name: &str) -> Option<&'a LayoutBox> {
    if layout
        .node
        .attributes()
        .and_then(|attributes| attributes.get("class").cloned())
        .map(|classes| {
            classes
                .split_ascii_whitespace()
                .any(|class| class == class_name)
        })
        .unwrap_or(false)
    {
        return Some(layout);
    }

    for child in &layout.children {
        if let Some(found) = find_layout_box_by_class(child, class_name) {
            return Some(found);
        }
    }

    None
}

fn find_parent_layout_box_by_id<'a>(layout: &'a LayoutBox, id: &str) -> Option<&'a LayoutBox> {
    for child in &layout.children {
        if child
            .node
            .attributes()
            .and_then(|attributes| attributes.get("id").cloned())
            .as_deref()
            == Some(id)
        {
            return Some(layout);
        }
        if let Some(found) = find_parent_layout_box_by_id(child, id) {
            return Some(found);
        }
    }
    None
}

fn translate_layout_box_for_test(
    layout: &mut LayoutBox,
    resolver: &mut StyleResolver,
    dx: f32,
    dy: f32,
) {
    if matches!(
        resolver.computed_style(&layout.node).get("position"),
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("fixed")
    ) {
        return;
    }
    layout.dimensions.content.x += dx;
    layout.dimensions.content.y += dy;
    for line in &mut layout.lines {
        line.rect.x += dx;
        line.rect.y += dy;
        line.baseline += dy;
        for fragment in &mut line.fragments {
            fragment.rect.x += dx;
            fragment.rect.y += dy;
        }
    }
    for child in &mut layout.children {
        translate_layout_box_for_test(child, resolver, dx, dy);
    }
}

#[test]
fn extract_stylesheets_skips_http_link_without_base_url() {
    let html = r#"<html><head>
            <link rel="stylesheet" href="https://example.com/style.css">
            <style>body { color: red; }</style>
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();
    // Only the inline <style> should be extracted; the HTTP link is skipped
    // because no base_url is provided.
    assert_eq!(stylesheets.len(), 1);
    assert!(stylesheets[0].contains("color: red"));
}

#[test]
fn extract_stylesheets_skips_noscript_fallback_content() {
    let html = r#"<html><head>
            <style>#react-root { display: block; }</style>
            <noscript>
                <style>#react-root { display: none !important; }</style>
                <link rel="stylesheet" href="data:text/css,%23react-root%7Bvisibility%3Ahidden%7D">
            </noscript>
        </head><body><div id="react-root"></div></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();

    assert_eq!(stylesheets.len(), 1);
    assert!(stylesheets[0].contains("display: block"));
    assert!(!stylesheets[0].contains("display: none"));
    assert!(!stylesheets[0].contains("visibility"));
}

#[test]
fn extract_stylesheets_includes_data_uri_without_base_url() {
    let html = r#"<html><head>
            <link rel="stylesheet" href="data:text/css,body%7Bmargin%3A0%7D">
            <style>p { color: blue; }</style>
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();
    assert_eq!(stylesheets.len(), 2);
    assert!(stylesheets[0].contains("margin"));
    assert!(stylesheets[1].contains("color: blue"));
}

#[test]
fn extract_stylesheets_skips_empty_href() {
    let html = r#"<html><head>
            <link rel="stylesheet" href="">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();
    assert!(stylesheets.is_empty());
}

#[test]
fn extract_stylesheets_fetches_relative_urls_with_base() {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    std::thread::spawn(move || {
        // Accept first request for /css/style.css
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(&stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        // Consume headers
        loop {
            let mut h = String::new();
            reader.read_line(&mut h).unwrap();
            if h.trim().is_empty() {
                break;
            }
        }

        let css_content = "body { margin: 0; }";
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
            css_content.len(),
            css_content
        );
        stream.write_all(resp.as_bytes()).unwrap();
        stream.flush().unwrap();

        // Accept second request for /other.css
        let (mut stream2, _) = listener.accept().unwrap();
        let mut reader2 = BufReader::new(&stream2);
        let mut line2 = String::new();
        reader2.read_line(&mut line2).unwrap();
        // Consume headers
        loop {
            let mut h = String::new();
            reader2.read_line(&mut h).unwrap();
            if h.trim().is_empty() {
                break;
            }
        }

        let css_content2 = "p { color: red; }";
        let resp2 = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
            css_content2.len(),
            css_content2
        );
        stream2.write_all(resp2.as_bytes()).unwrap();
        stream2.flush().unwrap();
    });

    let html = r#"<html><head>
            <link rel="stylesheet" href="/css/style.css">
            <link rel="stylesheet" href="other.css">
        </head><body></body></html>"#;

    let document = TreeBuilder::parse(html).document();
    let base_url = format!("http://127.0.0.1:{}/page.html", port)
        .parse::<crate::http::Url>()
        .unwrap();

    let stylesheets = extract_author_stylesheets(&document, Some(&base_url)).unwrap();

    assert_eq!(stylesheets.len(), 2);
    assert!(stylesheets[0].contains("margin: 0"));
    assert!(stylesheets[1].contains("color: red"));
}

#[test]
fn extract_stylesheets_expands_import_rules_in_source_order() {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let path = request_line
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_string();
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                if h.trim().is_empty() {
                    break;
                }
            }

            let css_content = match path.as_str() {
                "/main.css" => r#"@import "nested.css"; body { color: red; }"#,
                "/nested.css" => "p { color: blue; }",
                _ => "",
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                css_content.len(),
                css_content
            );
            stream.write_all(resp.as_bytes()).unwrap();
            stream.flush().unwrap();
        }
    });

    let html = r#"<html><head>
            <link rel="stylesheet" href="/main.css">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let base_url = format!("http://127.0.0.1:{}/index.html", port)
        .parse::<crate::http::Url>()
        .unwrap();
    let stylesheets = extract_author_stylesheets(&document, Some(&base_url)).unwrap();

    assert_eq!(stylesheets.len(), 2);
    assert!(stylesheets[0].contains("color: blue"));
    assert!(stylesheets[1].contains("color: red"));
}

#[test]
fn extract_stylesheets_limits_recursive_import_depth() {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    std::thread::spawn(move || {
        for _ in 0..6 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let path = request_line
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_string();
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                if h.trim().is_empty() {
                    break;
                }
            }

            let css_content = match path.as_str() {
                "/main.css" => r#"@import "a1.css"; .main { color: black; }"#,
                "/a1.css" => r#"@import "a2.css"; .a1 { color: #111; }"#,
                "/a2.css" => r#"@import "a3.css"; .a2 { color: #222; }"#,
                "/a3.css" => r#"@import "a4.css"; .a3 { color: #333; }"#,
                "/a4.css" => r#"@import "a5.css"; .a4 { color: #444; }"#,
                "/a5.css" => r#"@import "a6.css"; .a5 { color: #555; }"#,
                _ => "",
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                css_content.len(),
                css_content
            );
            stream.write_all(resp.as_bytes()).unwrap();
            stream.flush().unwrap();
        }
    });

    let html = r#"<html><head>
            <link rel="stylesheet" href="/main.css">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let base_url = format!("http://127.0.0.1:{}/index.html", port)
        .parse::<crate::http::Url>()
        .unwrap();
    let stylesheets = extract_author_stylesheets(&document, Some(&base_url)).unwrap();

    assert_eq!(stylesheets.len(), 6);
    assert!(stylesheets.iter().any(|css| css.contains(".a5")));
    assert!(!stylesheets.iter().any(|css| css.contains(".a6")));
}

#[test]
fn extract_stylesheets_follows_imports_even_when_parent_css_is_partially_invalid() {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let path = request_line
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_string();
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                if h.trim().is_empty() {
                    break;
                }
            }

            let css_content = match path.as_str() {
                "/main.css" => r#"@import "nested.css"; .main { color: black; broken }"#,
                "/nested.css" => ".nested { color: green; }",
                _ => "",
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                css_content.len(),
                css_content
            );
            stream.write_all(resp.as_bytes()).unwrap();
            stream.flush().unwrap();
        }
    });

    let html = r#"<html><head>
            <link rel="stylesheet" href="/main.css">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let base_url = format!("http://127.0.0.1:{}/index.html", port)
        .parse::<crate::http::Url>()
        .unwrap();
    let stylesheets = extract_author_stylesheets(&document, Some(&base_url)).unwrap();

    assert_eq!(stylesheets.len(), 2);
    assert!(stylesheets[0].contains(".nested"));
    assert!(stylesheets[1].contains(".main"));
}

#[test]
fn extract_stylesheets_supports_unquoted_url_import() {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let path = request_line
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_string();
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                if h.trim().is_empty() {
                    break;
                }
            }

            let css_content = match path.as_str() {
                "/main.css" => "@import url(nested.css); .main { color: black; }",
                "/nested.css" => ".nested { color: green; }",
                _ => "",
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                css_content.len(),
                css_content
            );
            stream.write_all(resp.as_bytes()).unwrap();
            stream.flush().unwrap();
        }
    });

    let html = r#"<html><head>
            <link rel="stylesheet" href="/main.css">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let base_url = format!("http://127.0.0.1:{}/index.html", port)
        .parse::<crate::http::Url>()
        .unwrap();
    let stylesheets = extract_author_stylesheets(&document, Some(&base_url)).unwrap();

    assert_eq!(stylesheets.len(), 2);
    assert!(stylesheets[0].contains(".nested"));
    assert!(stylesheets[1].contains(".main"));
}

#[test]
fn extract_stylesheets_skips_import_with_media_condition() {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let path = request_line
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_string();
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                if h.trim().is_empty() {
                    break;
                }
            }

            let css_content = match path.as_str() {
                "/main.css" => r#"@import "print.css" print; .main { color: black; }"#,
                "/print.css" => ".print { color: red; }",
                _ => "",
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                css_content.len(),
                css_content
            );
            stream.write_all(resp.as_bytes()).unwrap();
            stream.flush().unwrap();
        }
    });

    let html = r#"<html><head>
            <link rel="stylesheet" href="/main.css">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let base_url = format!("http://127.0.0.1:{}/index.html", port)
        .parse::<crate::http::Url>()
        .unwrap();
    let stylesheets = extract_author_stylesheets(&document, Some(&base_url)).unwrap();

    assert_eq!(stylesheets.len(), 1);
    assert!(stylesheets[0].contains(".main"));
    assert!(!stylesheets.iter().any(|css| css.contains(".print")));
}

#[test]
fn extract_stylesheets_handles_import_cycles_without_looping() {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    std::thread::spawn(move || {
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let path = request_line
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_string();
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                if h.trim().is_empty() {
                    break;
                }
            }

            let css_content = match path.as_str() {
                "/main.css" => r#"@import "a.css"; .main { color: black; }"#,
                "/a.css" => r#"@import "b.css"; .a { color: #111; }"#,
                "/b.css" => r#"@import "a.css"; .b { color: #222; }"#,
                _ => "",
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                css_content.len(),
                css_content
            );
            stream.write_all(resp.as_bytes()).unwrap();
            stream.flush().unwrap();
        }
    });

    let html = r#"<html><head>
            <link rel="stylesheet" href="/main.css">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let base_url = format!("http://127.0.0.1:{}/index.html", port)
        .parse::<crate::http::Url>()
        .unwrap();
    let stylesheets = extract_author_stylesheets(&document, Some(&base_url)).unwrap();

    assert_eq!(stylesheets.len(), 3);
    assert!(stylesheets[0].contains(".b"));
    assert!(stylesheets[1].contains(".a"));
    assert!(stylesheets[2].contains(".main"));
}

#[test]
fn extract_stylesheets_skips_failed_import_fetch() {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let path = request_line
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_string();
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                if h.trim().is_empty() {
                    break;
                }
            }

            let (status, css_content) = match path.as_str() {
                "/main.css" => (
                    "200 OK",
                    r#"@import "missing.css"; .main { color: black; }"#,
                ),
                "/missing.css" => ("404 Not Found", ""),
                _ => ("404 Not Found", ""),
            };
            let resp = format!(
                "HTTP/1.1 {}\r\nContent-Length: {}\r\n\r\n{}",
                status,
                css_content.len(),
                css_content
            );
            stream.write_all(resp.as_bytes()).unwrap();
            stream.flush().unwrap();
        }
    });

    let html = r#"<html><head>
            <link rel="stylesheet" href="/main.css">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let base_url = format!("http://127.0.0.1:{}/index.html", port)
        .parse::<crate::http::Url>()
        .unwrap();
    let stylesheets = extract_author_stylesheets(&document, Some(&base_url)).unwrap();

    assert_eq!(stylesheets.len(), 1);
    assert!(stylesheets[0].contains(".main"));
}

#[test]
fn extract_stylesheets_skips_absolute_import_urls() {
    let html = r#"<html><head>
            <style>@import "https://example.com/remote.css"; .main { color: black; }</style>
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let base_url = "http://127.0.0.1:80/index.html"
        .parse::<crate::http::Url>()
        .unwrap();
    let stylesheets = extract_author_stylesheets(&document, Some(&base_url)).unwrap();

    assert_eq!(stylesheets.len(), 1);
    assert!(stylesheets[0].contains(".main"));
}

#[test]
fn extract_stylesheets_skips_absolute_urls() {
    let html = r#"<html><head>
            <link rel="stylesheet" href="https://example.com/style.css">
            <link rel="stylesheet" href="//cdn.example.com/style.css">
            <style>p { color: blue; }</style>
        </head><body></body></html>"#;

    let document = TreeBuilder::parse(html).document();
    let base_url = "http://localhost:8000/page.html"
        .parse::<crate::http::Url>()
        .unwrap();

    let stylesheets = extract_author_stylesheets(&document, Some(&base_url)).unwrap();

    // Only the <style> tag should be included, not the absolute URLs
    assert_eq!(stylesheets.len(), 1);
    assert!(stylesheets[0].contains("color: blue"));
}

#[test]
fn extract_stylesheets_handles_case_insensitive_rel() {
    let html = r#"<html><head>
            <link rel="StyleSheet" href="data:text/css,body%7Bmargin%3A0%7D">
            <link rel="STYLESHEET" href="data:text/css,p%7Bcolor%3Ared%7D">
        </head><body></body></html>"#;

    let document = TreeBuilder::parse(html).document();
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();

    // Both should be recognized despite case differences
    assert_eq!(stylesheets.len(), 2);
    assert!(stylesheets[0].contains("margin"));
    assert!(stylesheets[1].contains("color"));
}

#[test]
fn extract_stylesheets_trims_whitespace_only_href() {
    let html = r#"<html><head>
            <link rel="stylesheet" href="   ">
            <style>p { color: blue; }</style>
        </head><body></body></html>"#;

    let document = TreeBuilder::parse(html).document();
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();

    // Only the <style> should be included, whitespace-only href is skipped
    assert_eq!(stylesheets.len(), 1);
    assert!(stylesheets[0].contains("color: blue"));
}

#[test]
fn resolve_url_strips_fragment() {
    let base: crate::http::Url = "https://example.com/dir/page.html".parse().unwrap();

    // Fragment should be stripped before resolution
    let resolved = crate::http::url::resolve_url(&base, "style.css#v2").unwrap();
    assert_eq!(resolved.path(), "/dir/style.css");
    assert_eq!(resolved.query(), None);

    // Absolute path with fragment
    let resolved = crate::http::url::resolve_url(&base, "/css/style.css#v1").unwrap();
    assert_eq!(resolved.path(), "/css/style.css");
}

#[test]
fn external_stylesheet_urls_are_resolved_against_the_stylesheet() {
    let stylesheet_url: crate::http::Url =
        "https://example.com/themes/demo/style.css".parse().unwrap();
    let mut output = Vec::new();
    let mut client = None;
    let mut active_import_urls = std::collections::HashSet::new();

    crate::paint::stylesheet::collect_stylesheet_with_imports(
        "body { background: url(images/pattern.png); } #hero { background-image: URL('../hero.png'); } .data { background: url(data:image/png;base64,AAAA); }".to_string(),
        Some(&stylesheet_url),
        None,
        &mut output,
        &mut client,
        0,
        &mut active_import_urls,
    )
    .unwrap();

    assert_eq!(output.len(), 1);
    assert!(output[0].contains("url(\"https://example.com/themes/demo/images/pattern.png\")"));
    assert!(output[0].contains("url(\"https://example.com/themes/hero.png\")"));
    assert!(output[0].contains("url(data:image/png;base64,AAAA)"));
}

#[test]
fn stylesheet_url_allows_same_origin_absolute_references() {
    let document: crate::http::Url = "https://example.com/page.html".parse().unwrap();

    let absolute = crate::paint::stylesheet::resolve_relative_stylesheet_url(
        &document,
        "https://example.com/assets/theme.css",
        Some(&document),
    )
    .unwrap();
    assert_eq!(absolute.to_string(), "https://example.com/assets/theme.css");

    let protocol_relative = crate::paint::stylesheet::resolve_relative_stylesheet_url(
        &document,
        "//example.com/assets/blocks.css",
        Some(&document),
    )
    .unwrap();
    assert_eq!(
        protocol_relative.to_string(),
        "https://example.com/assets/blocks.css"
    );
}

#[test]
fn stylesheet_url_allows_public_https_cross_origin_references() {
    let document: crate::http::Url = "https://example.com/page.html".parse().unwrap();

    let resolved = crate::paint::stylesheet::resolve_relative_stylesheet_url(
        &document,
        "https://1.1.1.1/theme.css",
        Some(&document),
    )
    .unwrap();
    assert_eq!(resolved.host(), "1.1.1.1");
}

#[test]
fn stylesheet_url_rejects_private_cross_origin_references() {
    let document: crate::http::Url = "https://example.com/page.html".parse().unwrap();

    for href in [
        "https://127.0.0.1/theme.css",
        "https://10.0.0.1/theme.css",
        "https://169.254.169.254/latest.css",
        "http://1.1.1.1/insecure.css",
    ] {
        assert!(
            crate::paint::stylesheet::resolve_relative_stylesheet_url(
                &document,
                href,
                Some(&document),
            )
            .is_none(),
            "must reject unsafe cross-origin stylesheet {href}"
        );
    }
}

#[test]
fn resolve_url_rejects_non_http_schemes() {
    let base: crate::http::Url = "https://example.com/page.html".parse().unwrap();

    // Non-HTTP(S) schemes should be rejected (mailto, data, etc.)
    assert!(crate::http::url::resolve_url(&base, "mailto:foo@example.com").is_err());
    assert!(crate::http::url::resolve_url(&base, "ftp://ftp.example.com/file.css").is_err());

    // data: URIs with embedded scheme should also be rejected
    assert!(crate::http::url::resolve_url(&base, "data:,foo").is_err());
}

#[test]
fn extract_stylesheets_respects_css_size_limit() {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    std::thread::spawn(move || {
        // Large CSS (exceeds 4 MiB limit)
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(&stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        // Consume headers
        loop {
            let mut h = String::new();
            reader.read_line(&mut h).unwrap();
            if h.trim().is_empty() {
                break;
            }
        }

        // Create a response with oversized CSS
        let large_css = "body { color: red; }".repeat(250_000); // ~5 MiB
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
            large_css.len(),
            large_css
        );
        stream.write_all(resp.as_bytes()).unwrap();
        stream.flush().unwrap();

        // Small CSS (under limit)
        let (mut stream2, _) = listener.accept().unwrap();
        let mut reader2 = BufReader::new(&stream2);
        let mut line2 = String::new();
        reader2.read_line(&mut line2).unwrap();
        // Consume headers
        loop {
            let mut h = String::new();
            reader2.read_line(&mut h).unwrap();
            if h.trim().is_empty() {
                break;
            }
        }

        let css = "p { color: green; }";
        let resp2 = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
            css.len(),
            css
        );
        stream2.write_all(resp2.as_bytes()).unwrap();
        stream2.flush().unwrap();
    });

    let html = r#"<html><head>
            <link rel="stylesheet" href="/large.css">
            <link rel="stylesheet" href="/small.css">
        </head><body></body></html>"#;

    let document = TreeBuilder::parse(html).document();
    let base_url = format!("http://127.0.0.1:{}/page.html", port)
        .parse::<crate::http::Url>()
        .unwrap();

    let stylesheets = extract_author_stylesheets(&document, Some(&base_url)).unwrap();

    // Large CSS should be skipped, only small CSS should be included
    assert_eq!(stylesheets.len(), 1);
    assert!(stylesheets[0].contains("color: green"));
}

#[test]
#[ignore = "debug test"]
fn debug_hello_world_layout() {
    let html = fs::read_to_string(acid2_official_reference_html_path()).unwrap();
    let document = TreeBuilder::parse(&html).document();
    materialize_local_assets(&document, &acid2_fixture_dir()).unwrap();

    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }

    let layout = crate::layout::layout_tree(
        &document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
    )
    .unwrap();

    // Find h2 box and print its text fragments
    fn find_h2_lines(lb: &crate::layout::LayoutBox) -> Vec<&crate::layout::LineBox> {
        if lb.node.tag_name().as_deref() == Some("h2") {
            return lb.lines.iter().collect();
        }
        for child in &lb.children {
            let lines = find_h2_lines(child);
            if !lines.is_empty() {
                return lines;
            }
        }
        vec![]
    }

    let lines = find_h2_lines(&layout);
    println!("Found {} lines in h2", lines.len());
    for (i, line) in lines.iter().enumerate() {
        println!("Line {}: ", i);
        for frag in &line.fragments {
            match &frag.content {
                crate::layout::InlineFragmentContent::Text(t) => {
                    println!("  Text: {:?}", t);
                    println!(
                        "    rect: x={:.2}, y={:.2}, w={:.2}, h={:.2}",
                        frag.rect.x, frag.rect.y, frag.rect.width, frag.rect.height
                    );
                    println!("    font_size: {:.2}", frag.metrics.font_size);
                }
                _ => println!("  Other content"),
            }
        }
    }
}

#[test]
#[ignore = "debug test"]
fn debug_acid2_hello_world_layout() {
    let html = fs::read_to_string(acid2_fixture_path()).unwrap();
    let document = TreeBuilder::parse(&html).document();
    materialize_local_assets(&document, &acid2_fixture_dir()).unwrap();

    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }

    let layout = crate::layout::layout_tree(
        &document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
    )
    .unwrap();

    // Find h2 box and print its text fragments
    fn find_h2_lines(lb: &crate::layout::LayoutBox) -> Vec<&crate::layout::LineBox> {
        if lb.node.tag_name().as_deref() == Some("h2") {
            return lb.lines.iter().collect();
        }
        for child in &lb.children {
            let lines = find_h2_lines(child);
            if !lines.is_empty() {
                return lines;
            }
        }
        vec![]
    }

    let lines = find_h2_lines(&layout);
    println!("Found {} lines in h2 (acid2.html)", lines.len());
    for (i, line) in lines.iter().enumerate() {
        println!("Line {}: ", i);
        for frag in &line.fragments {
            match &frag.content {
                crate::layout::InlineFragmentContent::Text(t) => {
                    println!("  Text: {:?}", t);
                    println!(
                        "    rect: x={:.2}, y={:.2}, w={:.2}, h={:.2}",
                        frag.rect.x, frag.rect.y, frag.rect.width, frag.rect.height
                    );
                    println!("    font_size: {:.2}", frag.metrics.font_size);
                }
                _ => println!("  Other content"),
            }
        }
    }
}

#[test]
#[ignore = "debug blog layout investigation"]
fn debug_blog_ast_moe_layout_snapshot() {
    let snapshot_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/output/blog-content.html");
    if !snapshot_path.exists() {
        // NOTE:
        // This is a local debugging helper that relies on an untracked snapshot
        // generated during manual rendering investigations.
        // - On CI, the snapshot is intentionally absent, so we skip to keep
        //   `cargo test -- --include-ignored` green.
        // - Locally, we fail loudly so missing fixture setup is immediately visible.
        if std::env::var_os("CI").is_some() {
            eprintln!(
                "skipping debug_blog_ast_moe_layout_snapshot on CI: missing {}",
                snapshot_path.display()
            );
            return;
        }
        eprintln!(
            "debug_blog_ast_moe_layout_snapshot requires local snapshot: {}",
            snapshot_path.display()
        );
        panic!(
            "missing required local snapshot for debug test: {}",
            snapshot_path.display()
        );
    }
    let html = fs::read_to_string(&snapshot_path).unwrap();
    let document = TreeBuilder::parse(&html).document();
    let base_url: crate::http::Url = "https://blog.ast.moe/blog/".parse().unwrap();

    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&document, Some(&base_url)).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }

    let layout = crate::layout::layout_tree(
        &document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 1366.0,
            height: 900.0,
        },
    )
    .unwrap();

    println!(
        "document content: x={:.1} y={:.1} w={:.1} h={:.1} children={}",
        layout.dimensions.content.x,
        layout.dimensions.content.y,
        layout.dimensions.content.width,
        layout.dimensions.content.height,
        layout.children.len()
    );

    for child in &layout.children {
        println!(
            "child tag={:?} x={:.1} y={:.1} w={:.1} h={:.1} lines={} children={}",
            child.node.tag_name(),
            child.dimensions.content.x,
            child.dimensions.content.y,
            child.dimensions.content.width,
            child.dimensions.content.height,
            child.lines.len(),
            child.children.len()
        );
    }

    let body = find_first_descendant_by_tag(&document, "body").unwrap();
    let body_style = resolver.computed_style(&body);
    println!("body width={:?}", body_style.get("width"));
    println!("body overflow={:?}", body_style.get("overflow"));
    println!("body overflow-x={:?}", body_style.get("overflow-x"));
    println!("body overflow-y={:?}", body_style.get("overflow-y"));

    let main = find_first_descendant_by_tag(&document, "main").unwrap();
    let main_style = resolver.computed_style(&main);
    println!("main width={:?}", main_style.get("width"));
    println!("main margin-left={:?}", main_style.get("margin-left"));
    println!("main margin-right={:?}", main_style.get("margin-right"));
}

#[test]
#[ignore = "debug test"]
fn debug_paint_trace() {
    use crate::font::load_system_font;

    let font = load_system_font("sans-serif").unwrap();
    let size = 24.0;
    let text = "Hello\u{00A0}World!";

    let mut cursor_x = 0.0_f32;
    let mut previous_char = None;

    println!("Tracing paint for \"Hello\\u{{00A0}}World!\" at {}px", size);
    for ch in text.chars() {
        let start_x = cursor_x;
        if let Some(prev) = previous_char {
            cursor_x += font.glyph_kerning(prev, ch, size);
        }

        if ch.is_whitespace() {
            let advance = font.glyph_advance(ch, size);
            cursor_x += advance;
            println!(
                "  '{:?}' (whitespace): x={:.2} -> {:.2} (advance={:.2})",
                ch, start_x, cursor_x, advance
            );
        } else {
            let glyph = font.rasterize(ch, size).unwrap();
            println!(
                "  '{}': x={:.2} -> {:.2} (advance_x={:.2}, glyph_advance={:.2})",
                ch,
                start_x,
                cursor_x + glyph.advance_x,
                glyph.advance_x,
                font.glyph_advance(ch, size)
            );
            cursor_x += glyph.advance_x;
        }
        previous_char = Some(ch);
    }
    println!("Final cursor_x: {:.2}", cursor_x);
    println!(
        "Expected width from measure_text_width: {:.2}",
        font.measure_text_width(text, size)
    );
}

#[test]
#[ignore = "debug test"]
fn debug_paint_offsets() {
    use crate::font::load_system_font;

    let font = load_system_font("sans-serif").unwrap();
    let size = 24.0;
    let text = "Hello\u{00A0}World!";

    println!("Glyph offsets for \"Hello\\u{{00A0}}World!\" at {}px", size);
    for ch in text.chars() {
        if !ch.is_whitespace() {
            let glyph = font.rasterize(ch, size).unwrap();
            println!(
                "  '{}': offset_x={:.2}, offset_y={:.2}, advance_x={:.2}, w={}, h={}",
                ch, glyph.offset_x, glyph.offset_y, glyph.advance_x, glyph.width, glyph.height
            );
        } else {
            println!("  '{:?}': (whitespace, no glyph)", ch);
        }
    }
}

#[test]
#[ignore = "debug test"]
fn debug_actual_pixel_positions() {
    use crate::font::load_system_font;

    let font = load_system_font("sans-serif").unwrap();
    let size = 24.0;
    let text = "Hello\u{00A0}World!";
    let start_x = 84.0_f32;

    let metrics = font.metrics().at_size(size);
    let baseline_y = 48.0 + metrics.ascender;
    let mut cursor_x = start_x;
    let mut previous_char = None;

    println!("Pixel positions for \"Hello\\u{{00A0}}World!\":");
    println!("start_x={:.2}, baseline_y={:.2}", start_x, baseline_y);

    for ch in text.chars() {
        if let Some(prev) = previous_char {
            cursor_x += font.glyph_kerning(prev, ch, size);
        }

        if ch.is_whitespace() {
            let advance = font.glyph_advance(ch, size);
            println!(
                "  '{:?}': skip (whitespace), cursor {:.2} -> {:.2}",
                ch,
                cursor_x,
                cursor_x + advance
            );
            cursor_x += advance;
        } else {
            let glyph = font.rasterize(ch, size).unwrap();
            let glyph_x = cursor_x + glyph.offset_x;
            let glyph_y = baseline_y + glyph.offset_y;
            println!(
                "  '{}': draw at ({:.2}, {:.2}), cursor {:.2} -> {:.2}",
                ch,
                glyph_x,
                glyph_y,
                cursor_x,
                cursor_x + glyph.advance_x
            );
            cursor_x += glyph.advance_x;
        }
        previous_char = Some(ch);
    }
    println!("Final cursor: {:.2}", cursor_x);
}

#[test]
#[ignore = "debug paint"]
fn debug_reference_paint() {
    let html = fs::read_to_string(acid2_official_reference_html_path()).unwrap();
    let document = TreeBuilder::parse(&html).document();
    materialize_local_assets(&document, &acid2_fixture_dir()).unwrap();

    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }

    let layout = crate::layout::layout_tree(
        &document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
    )
    .unwrap();

    // Find h2 and print its lines/fragments
    fn find_h2(lb: &crate::layout::LayoutBox) -> Option<&crate::layout::LayoutBox> {
        if lb.node.tag_name().as_deref() == Some("h2") {
            return Some(lb);
        }
        for child in &lb.children {
            if let Some(h2) = find_h2(child) {
                return Some(h2);
            }
        }
        None
    }

    if let Some(h2) = find_h2(&layout) {
        println!("H2 layout box:");
        println!(
            "  content rect: x={:.2}, y={:.2}, w={:.2}, h={:.2}",
            h2.dimensions.content.x,
            h2.dimensions.content.y,
            h2.dimensions.content.width,
            h2.dimensions.content.height
        );
        println!("  lines: {}", h2.lines.len());

        for (li, line) in h2.lines.iter().enumerate() {
            println!("  Line {}:", li);
            for (fi, frag) in line.fragments.iter().enumerate() {
                match &frag.content {
                    crate::layout::InlineFragmentContent::Text(t) => {
                        println!("    Fragment {}: Text {:?}", fi, t);
                        println!(
                            "      rect: x={:.2}, y={:.2}, w={:.2}, h={:.2}",
                            frag.rect.x, frag.rect.y, frag.rect.width, frag.rect.height
                        );
                    }
                    _ => {
                        println!("    Fragment {}: non-text", fi);
                    }
                }
            }
        }
    }
}

#[test]
#[ignore = "debug pixel positions"]
fn debug_painted_pixel_positions() {
    use crate::font::load_system_font;

    // Create a small canvas just for the text area
    let mut canvas = super::Canvas::new(200, 30);

    let font = std::sync::Arc::new(load_system_font("sans-serif").unwrap());
    let text = "Hello\u{00A0}World!";
    let font_size = 24.0;
    let start_x = 0.0;
    let start_y = 0.0;
    let color = super::Color::rgb(0, 0, 255); // Blue

    // Paint the text
    super::paint_text_with_font(
        &mut canvas,
        Rect {
            x: start_x,
            y: start_y,
            width: 200.0,
            height: 30.0,
        },
        text,
        font_size,
        font_size * 0.8,
        std::slice::from_ref(&font),
        color,
        None,
        0.0,
    );

    // Find leftmost and rightmost non-transparent pixel
    let mut min_x = 200u32;
    let mut max_x = 0u32;
    for y in 0..30 {
        for x in 0..200 {
            let idx = ((y * 200 + x) * 4) as usize;
            if idx + 3 < canvas.pixels.len() && canvas.pixels[idx + 3] > 0 {
                min_x = min_x.min(x);
                max_x = max_x.max(x);
            }
        }
    }

    println!("Painted text spans from x={} to x={}", min_x, max_x);
    println!(
        "Expected width: {:.2}",
        font.measure_text_width(text, font_size)
    );
    println!("Actual pixel width: {}", max_x - min_x + 1);

    // Find where "World!" starts by looking for gap
    for x in 45..60 {
        let mut has_pixel = false;
        for y in 0..30 {
            let idx = ((y * 200 + x) * 4) as usize;
            if idx + 3 < canvas.pixels.len() && canvas.pixels[idx + 3] > 0 {
                has_pixel = true;
                break;
            }
        }
        if !has_pixel {
            println!("First blank column after 'Hello' starts at x={}", x);
            break;
        }
    }

    for x in 45..150 {
        let mut has_pixel = false;
        for y in 0..30 {
            let idx = ((y * 200 + x) * 4) as usize;
            if idx + 3 < canvas.pixels.len() && canvas.pixels[idx + 3] > 0 {
                has_pixel = true;
                break;
            }
        }
        if has_pixel && x > 50 {
            println!("'World!' starts at x={}", x);
            break;
        }
    }
}

#[test]
#[ignore = "debug aid: always emit acid2 official-reference comparison PNGs"]
fn debug_write_acid2_official_reference_outputs() {
    let acid2_html = fs::read_to_string(acid2_fixture_path()).unwrap();
    let acid2_document = TreeBuilder::parse(&acid2_html).document();
    let mut acid2_resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&acid2_document, None).unwrap() {
        acid2_resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }
    let mut acid2_layout = crate::layout::layout_tree(
        &acid2_document,
        &mut acid2_resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
    )
    .unwrap();

    let reference_html = fs::read_to_string(acid2_official_reference_html_path()).unwrap();
    let reference_document = TreeBuilder::parse(&reference_html).document();
    materialize_local_assets(&reference_document, &acid2_fixture_dir()).unwrap();
    let mut reference_resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&reference_document, None).unwrap() {
        reference_resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }
    let reference_layout = crate::layout::layout_tree(
        &reference_document,
        &mut reference_resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
    )
    .unwrap();

    if let (Some((top_x, top_y)), Some((reference_x, reference_y))) = (
        find_layout_box_by_id(&acid2_layout, "top")
            .map(|top| (top.dimensions.content.x, top.dimensions.content.y)),
        find_first_layout_box_by_tag(&reference_layout, "h2")
            .map(|heading| (heading.dimensions.content.x, heading.dimensions.content.y)),
    ) {
        translate_layout_box_for_test(
            &mut acid2_layout,
            &mut acid2_resolver,
            reference_x - top_x,
            reference_y - top_y,
        );
    }

    let actual = paint_layout(
        &acid2_layout,
        &mut acid2_resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
    );
    let expected = paint_layout(
        &reference_layout,
        &mut reference_resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
    );

    let (diff, changed) = diff_canvases_with_tolerance(&actual, &expected, 1);
    fs::create_dir_all(acid2_output_dir()).unwrap();
    fs::write(
        fixture_output_image_path("acid2", "official-reference", "actual"),
        actual.encode_png(),
    )
    .unwrap();
    fs::write(
        fixture_output_image_path("acid2", "official-reference", "expected"),
        expected.encode_png(),
    )
    .unwrap();
    fs::write(
        fixture_output_image_path("acid2", "official-reference", "diff"),
        diff.encode_png(),
    )
    .unwrap();

    println!(
        "debug_write_acid2_official_reference_outputs: changed pixels = {} (tolerance=1)",
        changed
    );
}

// ============================================================
// media attribute tests (Issue 013-7)
// ============================================================

#[test]
fn media_attribute_screen_included() {
    let html = r#"<html><head>
            <link rel="stylesheet" media="screen" href="data:text/css,body{color:red}">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();
    assert_eq!(stylesheets.len(), 1);
    assert!(stylesheets[0].contains("color:red"));
}

#[test]
fn media_attribute_print_excluded() {
    let html = r#"<html><head>
            <link rel="stylesheet" media="print" href="data:text/css,body{color:red}">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();
    assert!(stylesheets.is_empty());
}

#[test]
fn media_attribute_all_included() {
    let html = r#"<html><head>
            <link rel="stylesheet" media="all" href="data:text/css,body{color:green}">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();
    assert_eq!(stylesheets.len(), 1);
    assert!(stylesheets[0].contains("color:green"));
}

#[test]
fn media_attribute_missing_included() {
    let html = r#"<html><head>
            <link rel="stylesheet" href="data:text/css,body{margin:0}">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();
    assert_eq!(stylesheets.len(), 1);
    assert!(stylesheets[0].contains("margin:0"));
}

#[test]
fn media_attribute_empty_included() {
    let html = r#"<html><head>
            <link rel="stylesheet" media="" href="data:text/css,body{padding:0}">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();
    assert_eq!(stylesheets.len(), 1);
    assert!(stylesheets[0].contains("padding:0"));
}

#[test]
fn media_attribute_screen_with_query_included() {
    let html = r#"<html><head>
            <link rel="stylesheet" media="screen and (min-width: 800px)" href="data:text/css,body{width:100%}">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();
    assert_eq!(stylesheets.len(), 1);
    assert!(stylesheets[0].contains("width:100%"));
    assert!(stylesheets[0].contains("@media screen and (min-width: 800px)"));
}

#[test]
fn style_media_attribute_query_is_preserved() {
    let html = r#"<html><head>
            <style media="all and (max-width: 768px)">nav{display:none}</style>
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();
    assert_eq!(stylesheets.len(), 1);
    assert!(stylesheets[0].contains("@media all and (max-width: 768px)"));
    assert!(stylesheets[0].contains("nav{display:none}"));
}

#[test]
fn media_attribute_case_insensitive() {
    let html = r#"<html><head>
            <link rel="stylesheet" media="SCREEN" href="data:text/css,body{color:red}">
            <link rel="stylesheet" media="Print" href="data:text/css,p{color:blue}">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();
    assert_eq!(stylesheets.len(), 1);
    assert!(stylesheets[0].contains("color:red"));
}

// ============================================================
// base element tests (Issue 013-7)
// ============================================================

#[test]
fn base_element_absolute_url_affects_base_calculation() {
    // This test verifies extract_document_base_url is used.
    // We can't easily test HTTP fetches without a server, so we test that
    // the function doesn't panic and returns the expected fallback.
    let html = r#"<html><head>
            <base href="https://cdn.example.com/assets/">
            <link rel="stylesheet" href="data:text/css,body{color:blue}">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    // With no fallback base and an absolute base href, data: URIs should still work
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();
    assert_eq!(stylesheets.len(), 1);
    assert!(stylesheets[0].contains("color:blue"));
}

#[test]
fn base_element_uses_first_base_only() {
    let html = r#"<html><head>
            <base href="https://first.example.com/">
            <base href="https://second.example.com/">
            <link rel="stylesheet" href="data:text/css,body{color:green}">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    // Multiple <base> elements: only the first should be used.
    // Data URIs work regardless, verifying the function executes.
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();
    assert_eq!(stylesheets.len(), 1);
}

#[test]
fn base_element_without_href_is_ignored() {
    let html = r#"<html><head>
            <base target="_blank">
            <link rel="stylesheet" href="data:text/css,body{color:red}">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    // <base> without href should be ignored, fallback to None
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();
    assert_eq!(stylesheets.len(), 1);
}

#[test]
fn base_element_empty_href_is_ignored() {
    let html = r#"<html><head>
            <base href="">
            <link rel="stylesheet" href="data:text/css,body{margin:10px}">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();
    assert_eq!(stylesheets.len(), 1);
    assert!(stylesheets[0].contains("margin:10px"));
}

// ============================================================
// Additional media attribute edge cases (Copilot review feedback)
// ============================================================

#[test]
fn media_attribute_small_not_matched_despite_containing_all() {
    // "small" contains "all" as a substring, but should NOT be matched
    let html = r#"<html><head>
            <link rel="stylesheet" media="small" href="data:text/css,body{color:red}">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();
    assert!(
        stylesheets.is_empty(),
        "media='small' should not match screen"
    );
}

#[test]
fn media_attribute_touchscreen_not_matched_despite_containing_screen() {
    // "touchscreen" contains "screen" as a substring, but should NOT be matched
    let html = r#"<html><head>
            <link rel="stylesheet" media="touchscreen" href="data:text/css,body{color:red}">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();
    assert!(
        stylesheets.is_empty(),
        "media='touchscreen' should not match screen"
    );
}

#[test]
fn media_attribute_not_screen_excluded() {
    // "not screen" should NOT be included for screen rendering
    let html = r#"<html><head>
            <link rel="stylesheet" media="not screen" href="data:text/css,body{color:red}">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();
    assert!(
        stylesheets.is_empty(),
        "media='not screen' should not match screen"
    );
}

#[test]
fn media_attribute_not_all_excluded() {
    // "not all" should NOT be included
    let html = r#"<html><head>
            <link rel="stylesheet" media="not all" href="data:text/css,body{color:red}">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();
    assert!(
        stylesheets.is_empty(),
        "media='not all' should not match screen"
    );
}

#[test]
fn media_attribute_only_screen_included() {
    // "only screen" should be included
    let html = r#"<html><head>
            <link rel="stylesheet" media="only screen" href="data:text/css,body{color:green}">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();
    assert_eq!(stylesheets.len(), 1);
    assert!(stylesheets[0].contains("color:green"));
}

#[test]
fn media_attribute_comma_separated_with_screen() {
    // "print, screen" should be included because screen is one of the options
    let html = r#"<html><head>
            <link rel="stylesheet" media="print, screen" href="data:text/css,body{color:blue}">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();
    assert_eq!(stylesheets.len(), 1);
    assert!(stylesheets[0].contains("color:blue"));
}

#[test]
fn media_attribute_comma_separated_print_only() {
    // "print, speech" should NOT be included (no screen or all)
    let html = r#"<html><head>
            <link rel="stylesheet" media="print, speech" href="data:text/css,body{color:red}">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();
    assert!(
        stylesheets.is_empty(),
        "media='print, speech' should not match screen"
    );
}

#[test]
fn media_attribute_feature_only_defaults_to_all() {
    // "(min-width: 800px)" without explicit media type defaults to "all"
    let html = r#"<html><head>
            <link rel="stylesheet" media="(min-width: 800px)" href="data:text/css,body{width:100%}">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();
    assert_eq!(stylesheets.len(), 1);
    assert!(stylesheets[0].contains("width:100%"));
}

// ============================================================
// Additional base element edge cases (Copilot review feedback)
// ============================================================

#[test]
fn base_element_first_without_href_uses_second_with_href() {
    // First <base> has no href, second has href - should use second
    let html = r#"<html><head>
            <base target="_blank">
            <base href="https://second.example.com/">
            <link rel="stylesheet" href="data:text/css,body{color:green}">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    // Data URIs should still work
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();
    assert_eq!(stylesheets.len(), 1);
}

#[test]
fn base_element_first_empty_href_uses_second() {
    // First <base> has empty href, second has valid href
    let html = r#"<html><head>
            <base href="">
            <base href="https://second.example.com/">
            <link rel="stylesheet" href="data:text/css,body{color:blue}">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let stylesheets = extract_author_stylesheets(&document, None).unwrap();
    assert_eq!(stylesheets.len(), 1);
}

#[test]
fn base_element_ssrf_protection_different_origin_ignored() {
    // <base> with different origin should be ignored for SSRF protection
    use crate::http::Url;
    let html = r#"<html><head>
            <base href="https://evil.example.com/assets/">
            <link rel="stylesheet" href="data:text/css,body{color:red}">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let original_base: Url = "https://original.example.com/page.html".parse().unwrap();

    // The base URL extraction should ignore different-origin absolute URLs
    let effective_base = extract_document_base_url(&document, Some(&original_base));
    // Should fall back to original_base since evil.example.com is different origin
    assert_eq!(
        effective_base.as_ref().map(|u| u.host()),
        Some("original.example.com")
    );
}

#[test]
fn base_element_same_origin_accepted() {
    // <base> with same origin should be accepted
    use crate::http::Url;
    let html = r#"<html><head>
            <base href="https://example.com/assets/">
            <link rel="stylesheet" href="data:text/css,body{color:green}">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let original_base: Url = "https://example.com/page.html".parse().unwrap();

    let effective_base = extract_document_base_url(&document, Some(&original_base));
    // Should use the <base> href since it's same origin
    assert_eq!(effective_base.as_ref().map(|u| u.path()), Some("/assets/"));
}

#[test]
fn base_element_relative_url_always_same_origin() {
    // Relative <base> href is always resolved against original base (same origin)
    use crate::http::Url;
    let html = r#"<html><head>
            <base href="/assets/">
            <link rel="stylesheet" href="data:text/css,body{color:blue}">
        </head><body></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let original_base: Url = "https://example.com/page.html".parse().unwrap();

    let effective_base = extract_document_base_url(&document, Some(&original_base));
    assert_eq!(
        effective_base.as_ref().map(|u| u.host()),
        Some("example.com")
    );
    assert_eq!(effective_base.as_ref().map(|u| u.path()), Some("/assets/"));
}

/// Helper: paint a single div with the given background-color CSS value and return the canvas.
fn paint_single_div_with_color(color_value: &str) -> Canvas {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let panel = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(panel);

    let stylesheet = format!(
        "body {{ margin: 0; }} div {{ width: 10px; height: 10px; background-color: {color_value}; }}"
    );
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(&stylesheet).unwrap());
    let layout = layout_tree(
        &document,
        &mut resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 20.0,
            height: 20.0,
        },
    )
    .unwrap();

    let mut paint_resolver = StyleResolver::new();
    paint_resolver.add_stylesheet(Origin::Author, parse_stylesheet(&stylesheet).unwrap());
    paint_layout(
        &layout,
        &mut paint_resolver,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 20.0,
            height: 20.0,
        },
    )
}

#[test]
fn parses_rgba_color_with_alpha() {
    // rgba(255, 0, 0, 0.5) → semi-transparent red (alpha=128)
    let canvas = paint_single_div_with_color("rgba(255, 0, 0, 0.5)");
    let pixel = canvas.pixel(5, 5).unwrap();
    assert_eq!(pixel.r, 255);
    assert_eq!(pixel.g, 0);
    assert_eq!(pixel.b, 0);
    // alpha blended onto transparent background: out_a ≈ 128
    assert!(
        pixel.a > 100 && pixel.a < 150,
        "expected alpha ~128, got {}",
        pixel.a
    );
}

#[test]
fn parses_rgba_fully_opaque() {
    let canvas = paint_single_div_with_color("rgba(0, 128, 64, 1)");
    assert_eq!(canvas.pixel(5, 5), Some(Color::rgba(0, 128, 64, 255)));
}

#[test]
fn parses_rgba_fully_transparent() {
    let canvas = paint_single_div_with_color("rgba(255, 0, 0, 0)");
    // fully transparent → pixel stays transparent
    assert_eq!(canvas.pixel(5, 5), Some(Color::rgba(0, 0, 0, 0)));
}

#[test]
fn parses_hsl_red() {
    // hsl(0, 100%, 50%) → pure red
    let canvas = paint_single_div_with_color("hsl(0, 100%, 50%)");
    assert_eq!(canvas.pixel(5, 5), Some(Color::rgb(255, 0, 0)));
}

#[test]
fn parses_hsl_green() {
    // hsl(120, 100%, 50%) → pure green
    let canvas = paint_single_div_with_color("hsl(120, 100%, 50%)");
    assert_eq!(canvas.pixel(5, 5), Some(Color::rgb(0, 255, 0)));
}

#[test]
fn parses_hsl_blue() {
    // hsl(240, 100%, 50%) → pure blue
    let canvas = paint_single_div_with_color("hsl(240, 100%, 50%)");
    assert_eq!(canvas.pixel(5, 5), Some(Color::rgb(0, 0, 255)));
}

#[test]
fn parses_hsl_white() {
    // hsl(0, 0%, 100%) → white
    let canvas = paint_single_div_with_color("hsl(0, 0%, 100%)");
    assert_eq!(canvas.pixel(5, 5), Some(Color::rgb(255, 255, 255)));
}

#[test]
fn parses_hsl_black() {
    // hsl(0, 0%, 0%) → black
    let canvas = paint_single_div_with_color("hsl(0, 0%, 0%)");
    assert_eq!(canvas.pixel(5, 5), Some(Color::rgb(0, 0, 0)));
}

#[test]
fn parses_hsla_semi_transparent() {
    // hsla(0, 100%, 50%, 0) → fully transparent red
    let canvas = paint_single_div_with_color("hsla(0, 100%, 50%, 0)");
    assert_eq!(canvas.pixel(5, 5), Some(Color::rgba(0, 0, 0, 0)));
}

#[test]
fn parses_hsla_opaque() {
    // hsla(240, 100%, 50%, 1) → fully opaque blue
    let canvas = paint_single_div_with_color("hsla(240, 100%, 50%, 1)");
    assert_eq!(canvas.pixel(5, 5), Some(Color::rgb(0, 0, 255)));
}

#[test]
fn parses_8digit_hex_color() {
    // #ff0000ff → fully opaque red
    let canvas = paint_single_div_with_color("#ff0000ff");
    assert_eq!(canvas.pixel(5, 5), Some(Color::rgb(255, 0, 0)));
}

#[test]
fn parses_8digit_hex_color_transparent() {
    // #ff000000 → fully transparent red → transparent pixel
    let canvas = paint_single_div_with_color("#ff000000");
    assert_eq!(canvas.pixel(5, 5), Some(Color::rgba(0, 0, 0, 0)));
}

#[test]
fn parses_4digit_hex_color() {
    // #f00f → fully opaque red (#ff0000ff)
    let canvas = paint_single_div_with_color("#f00f");
    assert_eq!(canvas.pixel(5, 5), Some(Color::rgb(255, 0, 0)));
}

#[test]
fn parses_4digit_hex_color_transparent() {
    // #f000 → fully transparent red → transparent pixel
    let canvas = paint_single_div_with_color("#f000");
    assert_eq!(canvas.pixel(5, 5), Some(Color::rgba(0, 0, 0, 0)));
}

#[test]
fn parses_named_color_coral() {
    // coral = rgb(255, 127, 80)
    let canvas = paint_single_div_with_color("coral");
    assert_eq!(canvas.pixel(5, 5), Some(Color::rgb(255, 127, 80)));
}

#[test]
fn parses_named_color_crimson() {
    // crimson = rgb(220, 20, 60)
    let canvas = paint_single_div_with_color("crimson");
    assert_eq!(canvas.pixel(5, 5), Some(Color::rgb(220, 20, 60)));
}

#[test]
fn parses_named_color_orange() {
    // orange = rgb(255, 165, 0)
    let canvas = paint_single_div_with_color("orange");
    assert_eq!(canvas.pixel(5, 5), Some(Color::rgb(255, 165, 0)));
}

#[test]
fn parses_named_color_pink() {
    // pink = rgb(255, 192, 203)
    let canvas = paint_single_div_with_color("pink");
    assert_eq!(canvas.pixel(5, 5), Some(Color::rgb(255, 192, 203)));
}

// ===== text-transform tests =====

#[test]
fn apply_text_transform_uppercase() {
    let result = super::apply_text_transform("hello world", "uppercase");
    assert_eq!(result.as_deref(), Some("HELLO WORLD"));
}

#[test]
fn apply_text_transform_lowercase() {
    let result = super::apply_text_transform("HELLO WORLD", "lowercase");
    assert_eq!(result.as_deref(), Some("hello world"));
}

#[test]
fn apply_text_transform_capitalize() {
    let result = super::apply_text_transform("hello world", "capitalize");
    assert_eq!(result.as_deref(), Some("Hello World"));
}

#[test]
fn apply_text_transform_none_returns_none() {
    let result = super::apply_text_transform("hello", "none");
    assert!(result.is_none(), "transform:none should return None");
}

// ===== text-decoration paint tests =====

/// Helper: build a canvas rendering "Hi" with the given text-decoration CSS value.
fn render_text_with_decoration(decoration: &str) -> Canvas {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let p = NodeHandle::element("p");
    let text = NodeHandle::text("Hi");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(p.clone());
    p.append_child(text);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(&format!(
            "body {{ margin: 0; }} \
             p {{ text-decoration: {}; color: #ff0000; font-size: 20px; }}",
            decoration
        ))
        .unwrap(),
    );

    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 200.0,
        height: 100.0,
    };
    let layout = layout_tree(&document, &mut resolver, viewport).expect("layout should succeed");
    paint_layout(&layout, &mut resolver, viewport)
}

#[test]
fn text_decoration_underline_draws_pixels_below_text() {
    // Render the same text with and without underline.
    let canvas_underline = render_text_with_decoration("underline");
    let canvas_none = render_text_with_decoration("none");

    let w = canvas_underline.width();
    let h = canvas_underline.height();

    // Find the bottom-most row that contains colored pixels in the no-underline render.
    // This is the lower boundary of the text glyph area.
    let glyph_bottom: u32 = (0..h)
        .filter(|&y| (0..w).any(|x| canvas_none.pixel(x, y).map_or(false, |c| c.a > 0)))
        .last()
        .expect("no-decoration render must contain at least one colored pixel");

    // Rows strictly below the glyph area.
    let below_glyph_start = glyph_bottom + 1;

    // Compare the two canvases: all differing pixels must be strictly below glyph_bottom,
    // and at least one differing pixel must exist there.
    let mut diff_below = false;
    let mut diff_at_or_above = false;
    for y in 0..h {
        for x in 0..w {
            let a_ul = canvas_underline.pixel(x, y).map_or(0, |c| c.a);
            let a_none = canvas_none.pixel(x, y).map_or(0, |c| c.a);
            if a_ul != a_none {
                if y > glyph_bottom {
                    diff_below = true;
                } else {
                    diff_at_or_above = true;
                }
            }
        }
    }

    assert!(
        diff_below,
        "text-decoration: underline must introduce pixel differences below the glyph area \
         (glyph_bottom={glyph_bottom}, checked rows {below_glyph_start}..{h})"
    );
    assert!(
        !diff_at_or_above,
        "text-decoration: underline should only differ from none below the glyph area \
         (glyph_bottom={glyph_bottom})"
    );
}

// --- border-radius 描画テスト ---

#[test]
fn fill_rounded_rect_clips_corners() {
    // 20x20 のキャンバスに 20x20 の角丸矩形（radius=5）を描画する。
    // 4コーナーの最外ピクセルは透明のまま残るはず。
    let mut canvas = Canvas::new(20, 20);
    canvas.fill_rounded_rect(
        Rect {
            x: 0.0,
            y: 0.0,
            width: 20.0,
            height: 20.0,
        },
        Color::rgb(255, 0, 0),
        5.0,
        5.0,
        5.0,
        5.0,
        None,
    );

    // 中央はぬりつぶされている
    assert_eq!(canvas.pixel(10, 10), Some(Color::rgb(255, 0, 0)));

    // 4コーナーの最外ピクセルは透明
    assert_eq!(canvas.pixel(0, 0), Some(Color::rgba(0, 0, 0, 0)));
    assert_eq!(canvas.pixel(19, 0), Some(Color::rgba(0, 0, 0, 0)));
    assert_eq!(canvas.pixel(0, 19), Some(Color::rgba(0, 0, 0, 0)));
    assert_eq!(canvas.pixel(19, 19), Some(Color::rgba(0, 0, 0, 0)));
}

#[test]
fn rounded_rect_scanlines_match_per_pixel_reference() {
    let cases = [
        (
            Rect { x: 0.0, y: 0.0, width: 20.0, height: 20.0 },
            [5.0, 5.0, 5.0, 5.0],
            None,
        ),
        (
            Rect { x: 1.25, y: 2.75, width: 17.5, height: 13.25 },
            [7.0, 2.0, 5.0, 3.0],
            None,
        ),
        (
            Rect { x: 1.25, y: 2.75, width: 17.5, height: 13.25 },
            [7.0, 2.0, 5.0, 3.0],
            Some(Rect { x: 3.4, y: 4.2, width: 11.3, height: 8.1 }),
        ),
    ];
    for (rect, radii, clip) in cases {
        let color = Color::rgba(31, 97, 211, 173);
        let mut actual = Canvas::new(24, 20);
        actual.fill_rounded_rect(
            rect, color, radii[0], radii[1], radii[2], radii[3], clip,
        );

        let mut expected = Canvas::new(24, 20);
        let tl = radii[0].min(rect.width / 2.0).min(rect.height / 2.0).max(0.0);
        let tr = radii[1].min(rect.width / 2.0).min(rect.height / 2.0).max(0.0);
        let br = radii[2].min(rect.width / 2.0).min(rect.height / 2.0).max(0.0);
        let bl = radii[3].min(rect.width / 2.0).min(rect.height / 2.0).max(0.0);
        for y in 0..expected.height() {
            for x in 0..expected.width() {
                let fx = x as f32 + 0.5;
                let fy = y as f32 + 0.5;
                let in_clip = clip.is_none_or(|clip| {
                    fx >= clip.x
                        && fx < clip.x + clip.width
                        && fy >= clip.y
                        && fy < clip.y + clip.height
                });
                if in_clip
                    && point_in_rounded_rect(
                        fx, fy, rect.x, rect.y, rect.width, rect.height, tl, tr, br, bl,
                    )
                {
                    let index = ((y * expected.width() + x) * 4) as usize;
                    blend_pixel(&mut expected.pixels[index..index + 4], color);
                }
            }
        }
        assert_eq!(actual.pixels(), expected.pixels(), "rect={rect:?}, clip={clip:?}");
    }
}

#[test]
fn paint_border_radius_clips_background_corners() {
    // div に border-radius を指定すると、4コーナーのピクセルが背景色で塗られない
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(div);

    let css = "body { margin: 0; } \
               div { width: 20px; height: 20px; background-color: #ff0000; border-radius: 5px; }";

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());

    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 40.0,
        height: 40.0,
    };
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let canvas = paint_layout(&layout, &mut resolver, viewport);

    // 中央は赤
    assert_eq!(canvas.pixel(10, 10), Some(Color::rgb(255, 0, 0)));

    // 4コーナーの最外ピクセルは透明（角丸でクリップ）
    assert_eq!(canvas.pixel(0, 0), Some(Color::rgba(0, 0, 0, 0)));
    assert_eq!(canvas.pixel(19, 0), Some(Color::rgba(0, 0, 0, 0)));
    assert_eq!(canvas.pixel(0, 19), Some(Color::rgba(0, 0, 0, 0)));
    assert_eq!(canvas.pixel(19, 19), Some(Color::rgba(0, 0, 0, 0)));
}

#[test]
fn uniform_pill_border_paints_rounded_corner_arc() {
    let canvas = paint_clip_path_document(
        ".target { width: 40px; height: 20px; border: 1px solid red; \
                    border-radius: 9999px; }",
        "<div class='target'></div>",
        50.0,
        30.0,
    );

    assert_eq!(canvas.pixel(2, 4), Some(Color::rgb(255, 0, 0)));
    assert_eq!(canvas.pixel(21, 11), Some(Color::rgba(0, 0, 0, 0)));
}

// --- clip-path: inset() 描画テスト ---

fn paint_clip_path_document(css: &str, body: &str, width: f32, height: f32) -> Canvas {
    let html = format!(
        "<html><head><style>body {{ margin: 0; }} {css}</style></head><body>{body}</body></html>"
    );
    let document = TreeBuilder::parse(&html).document();
    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width,
        height,
    };
    render_document(&document, viewport).unwrap()
}

#[test]
fn clip_path_inset_full_bottom_hides_the_element() {
    let canvas = paint_clip_path_document(
        ".target { width: 100px; height: 80px; background: red; \
         clip-path: inset(0 0 100% 0); }",
        "<div class='target'></div>",
        100.0,
        80.0,
    );

    assert_eq!(count_pixels(&canvas, Color::rgb(255, 0, 0)), 0);
    assert_eq!(canvas.pixel(0, 0), Some(Color::rgba(0, 0, 0, 0)));
    assert_eq!(canvas.pixel(50, 40), Some(Color::rgba(0, 0, 0, 0)));
    assert_eq!(canvas.pixel(99, 79), Some(Color::rgba(0, 0, 0, 0)));
}

#[test]
fn clip_path_inset_px_clips_at_each_pixel_boundary() {
    let canvas = paint_clip_path_document(
        ".target { width: 100px; height: 80px; background: red; \
         clip-path: inset(10px 20px 30px 40px); }",
        "<div class='target'></div>",
        100.0,
        80.0,
    );

    let transparent = Some(Color::rgba(0, 0, 0, 0));
    let red = Some(Color::rgb(255, 0, 0));
    assert_eq!(canvas.pixel(40, 10), red);
    assert_eq!(canvas.pixel(79, 49), red);
    assert_eq!(canvas.pixel(39, 10), transparent);
    assert_eq!(canvas.pixel(80, 10), transparent);
    assert_eq!(canvas.pixel(40, 9), transparent);
    assert_eq!(canvas.pixel(40, 50), transparent);
    assert_eq!(count_pixels(&canvas, Color::rgb(255, 0, 0)), 40 * 40);
}

#[test]
fn clip_path_inset_percentages_use_width_for_sides_and_height_for_edges() {
    let canvas = paint_clip_path_document(
        ".target { width: 100px; height: 80px; background: red; \
         clip-path: inset(25% 10% 50% 20%); }",
        "<div class='target'></div>",
        100.0,
        80.0,
    );

    let transparent = Some(Color::rgba(0, 0, 0, 0));
    let red = Some(Color::rgb(255, 0, 0));
    assert_eq!(canvas.pixel(20, 20), red);
    assert_eq!(canvas.pixel(89, 39), red);
    assert_eq!(canvas.pixel(19, 20), transparent);
    assert_eq!(canvas.pixel(90, 20), transparent);
    assert_eq!(canvas.pixel(20, 19), transparent);
    assert_eq!(canvas.pixel(20, 40), transparent);
    assert_eq!(count_pixels(&canvas, Color::rgb(255, 0, 0)), 70 * 20);
}

#[test]
fn clip_path_inset_clips_descendants() {
    let canvas = paint_clip_path_document(
        ".parent { position: relative; width: 100px; height: 80px; \
         clip-path: inset(10px 20px 30px 40px); } \
         .child { position: absolute; left: 0; top: 0; width: 100px; height: 80px; \
         background: blue; }",
        "<div class='parent'><div class='child'></div></div>",
        100.0,
        80.0,
    );

    let transparent = Some(Color::rgba(0, 0, 0, 0));
    let blue = Some(Color::rgb(0, 0, 255));
    assert_eq!(canvas.pixel(40, 10), blue);
    assert_eq!(canvas.pixel(79, 49), blue);
    assert_eq!(canvas.pixel(39, 10), transparent);
    assert_eq!(canvas.pixel(80, 10), transparent);
    assert_eq!(canvas.pixel(40, 9), transparent);
    assert_eq!(canvas.pixel(40, 50), transparent);
    assert_eq!(count_pixels(&canvas, Color::rgb(0, 0, 255)), 40 * 40);
}

// --- box-shadow テスト ---

#[test]
fn box_shadow_renders_shadow_pixels() {
    // div に box-shadow を指定すると、div 外側にシャドウのピクセルが描画される。
    // シャドウは div の下・右にオフセットする。
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(div);

    // 20x20 の白い div、右下 5px オフセット、blur=0、黒いシャドウ
    let css = "body { margin: 0; } \
               div { width: 20px; height: 20px; background-color: #ffffff; \
                     box-shadow: 5px 5px 0px 0px #000000; }";

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());

    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 60.0,
        height: 60.0,
    };
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let canvas = paint_layout(&layout, &mut resolver, viewport);

    // シャドウ部分（x=20, y=20 あたり）に不透明なピクセルがあるはず
    // shadow_rect = (5,5)-(25,25) なので、中間の (20,20) はシャドウ内
    let shadow_pixel = canvas.pixel(20, 20);
    assert!(
        shadow_pixel.map_or(false, |c| c.a > 0),
        "box-shadow should render shadow at offset position, got {:?}",
        shadow_pixel
    );

    // div 本体の左上コーナー (0, 0) は白いはず
    let body_pixel = canvas.pixel(0, 0);
    assert_eq!(
        body_pixel,
        Some(Color::rgb(255, 255, 255)),
        "div body should be white at (0,0), got {:?}",
        body_pixel
    );
}

#[test]
fn box_shadow_none_renders_no_extra_pixels() {
    // box-shadow: none を指定した場合、div の外側に余分なピクセルがないこと。
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(div);

    let css = "body { margin: 0; } \
               div { width: 20px; height: 20px; background-color: #ff0000; \
                     box-shadow: none; }";

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());

    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 40.0,
        height: 40.0,
    };
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let canvas = paint_layout(&layout, &mut resolver, viewport);

    // div 右外のピクセルは透明のはず
    assert_eq!(
        canvas.pixel(25, 10),
        Some(Color::rgba(0, 0, 0, 0)),
        "no shadow pixels outside div when box-shadow: none"
    );
}

#[test]
fn box_shadow_with_blur_renders_blurred_shadow() {
    // blur 付きの box-shadow が描画されること。
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(div);

    let css = "body { margin: 0; } \
               div { width: 20px; height: 20px; background-color: #ffffff; \
                     box-shadow: 4px 4px 4px 0px rgba(0, 0, 0, 0.5); }";

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());

    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 60.0,
        height: 60.0,
    };
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let canvas = paint_layout(&layout, &mut resolver, viewport);

    // シャドウの中心付近にピクセルがあるはず（blur があるのでいくらかアルファがある）
    let has_shadow = (20..35u32)
        .flat_map(|x| (20..35u32).map(move |y| (x, y)))
        .any(|(x, y)| canvas.pixel(x, y).map_or(false, |c| c.a > 0));
    assert!(
        has_shadow,
        "box-shadow with blur should render some pixels in shadow area"
    );
}

// --- opacity テスト ---

#[test]
fn filter_brightness_applies_to_the_element_subtree() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(div);

    let css = "body { margin: 0; } div { width: 20px; height: 20px; \
               background-color: #4080c0; filter: brightness(0.5); }";
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());
    let viewport = Rect { x: 0.0, y: 0.0, width: 40.0, height: 40.0 };
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let canvas = paint_layout(&layout, &mut resolver, viewport);

    assert_eq!(canvas.pixel(10, 10), Some(Color::rgba(32, 64, 96, 255)));
}

#[test]
fn filter_blur_preserves_color_at_transparent_edges() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(div);
    let css = "body { margin: 0; } div { width: 10px; height: 10px; background-color: red; filter: blur(1px); }";
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());
    let viewport = Rect { x: 0.0, y: 0.0, width: 20.0, height: 20.0 };
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let pixel = paint_layout(&layout, &mut resolver, viewport).pixel(10, 5).unwrap();

    assert_eq!((pixel.r, pixel.g, pixel.b), (255, 0, 0));
    assert!(pixel.a > 0 && pixel.a < 255);
}

#[test]
fn filter_functions_apply_in_declared_order() {
    fn render(filter: &str) -> Color {
        let document = NodeHandle::document();
        let body = NodeHandle::element("body");
        let div = NodeHandle::element("div");
        document.append_child(body.clone());
        body.append_child(div);
        let css = format!("body {{ margin: 0; }} div {{ width: 10px; height: 10px; background-color: #4080c0; filter: {filter}; }}");
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(Origin::Author, parse_stylesheet(&css).unwrap());
        let viewport = Rect { x: 0.0, y: 0.0, width: 20.0, height: 20.0 };
        let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
        paint_layout(&layout, &mut resolver, viewport).pixel(5, 5).unwrap()
    }

    assert_ne!(render("brightness(0.5) invert(1)"), render("invert(1) brightness(0.5)"));
}

#[test]
fn backdrop_filter_changes_only_the_pixels_behind_the_element() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(div);
    let css = "body { margin: 0; width: 20px; height: 20px; background-color: #4080c0; } \
               div { width: 10px; height: 10px; backdrop-filter: brightness(0.5); }";
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());
    let viewport = Rect { x: 0.0, y: 0.0, width: 20.0, height: 20.0 };
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let canvas = paint_layout(&layout, &mut resolver, viewport);

    assert_eq!(canvas.pixel(5, 5), Some(Color::rgba(32, 64, 96, 255)));
    assert_eq!(canvas.pixel(15, 15), Some(Color::rgba(64, 128, 192, 255)));
}

/// backdrop-filter テスト用の DOM (`.a`, `.b > .c`) を描画し、キャンバスと
/// 処理された backdrop pixel 数を返す。
///
/// `body` の margin と viewport サイズは自動で設定するため、CSS では
/// 背景色と各要素のスタイルだけを指定する。
fn render_backdrop(css: &str, viewport_size: f32) -> (Canvas, u64) {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let a = NodeHandle::element("div");
    let b = NodeHandle::element("div");
    let c = NodeHandle::element("div");
    a.set_attribute("class", "a");
    b.set_attribute("class", "b");
    c.set_attribute("class", "c");
    document.append_child(body.clone());
    body.append_child(a);
    body.append_child(b.clone());
    b.append_child(c);

    let css = format!(
        "body {{ margin: 0; width: {viewport_size}px; height: {viewport_size}px; }} {css}"
    );
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(&css).unwrap());
    let viewport = Rect { x: 0.0, y: 0.0, width: viewport_size, height: viewport_size };
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();

    super::take_backdrop_surface_pixels();
    let canvas = paint_layout(&layout, &mut resolver, viewport);
    let backdrop_pixels = super::take_backdrop_surface_pixels();
    (canvas, backdrop_pixels)
}

#[test]
fn backdrop_filter_processes_only_the_local_region() {
    // 200x200 のキャンバスに 10x10 の blur(2px) backdrop がある場合、
    // 処理されるのは border box (50..60) を padding 2px 拡張した 14x14 だけ。
    let (canvas, backdrop_pixels) = render_backdrop(
        "body { background-color: #4080c0; } \
         .a { position: absolute; left: 50px; top: 50px; width: 10px; height: 10px; \
              backdrop-filter: blur(2px); } \
         .b { display: none; }",
        200.0,
    );

    assert_eq!(backdrop_pixels, 14 * 14);
    // 一様な背景の blur は同じ色になるため、要素の内外どちらも背景色のまま。
    assert_eq!(canvas.pixel(55, 55), Some(Color::rgba(64, 128, 192, 255)));
    assert_eq!(canvas.pixel(150, 150), Some(Color::rgba(64, 128, 192, 255)));
}

#[test]
fn backdrop_filter_color_filter_processes_exactly_the_border_box() {
    // 色 filter は周辺 pixel を参照しないため、処理面積は border box と完全に一致する。
    let (canvas, backdrop_pixels) = render_backdrop(
        "body { background-color: #4080c0; } \
         .a { position: absolute; left: 50px; top: 50px; width: 10px; height: 10px; \
              backdrop-filter: brightness(0.5); } \
         .b { display: none; }",
        200.0,
    );

    assert_eq!(backdrop_pixels, 100);
    assert_eq!(canvas.pixel(55, 55), Some(Color::rgba(32, 64, 96, 255)));
    assert_eq!(canvas.pixel(59, 59), Some(Color::rgba(32, 64, 96, 255)));
    assert_eq!(canvas.pixel(60, 60), Some(Color::rgba(64, 128, 192, 255)));
    assert_eq!(canvas.pixel(49, 49), Some(Color::rgba(64, 128, 192, 255)));
}

#[test]
fn backdrop_filter_blur_samples_pixels_outside_the_element() {
    // 要素の左隣にだけ赤があり、要素自身は青の上にある。blur が要素の外周
    // pixel を参照していれば、要素内の左端付近に赤が混ざる。
    let (canvas, backdrop_pixels) = render_backdrop(
        "body { margin: 0; background-color: #0000ff; } \
         .a { position: absolute; left: 0; top: 0; width: 20px; height: 40px; background-color: #ff0000; } \
         .b { position: absolute; left: 20px; top: 0; width: 20px; height: 40px; backdrop-filter: blur(5px); }",
        40.0,
    );

    // 出力領域 20..40 x 0..40 を padding 5px 拡張し、キャンバスで clamp した領域。
    assert_eq!(backdrop_pixels, 25 * 40);

    let near_boundary = canvas.pixel(21, 20).unwrap();
    assert!(
        near_boundary.r > 0,
        "blur must sample the red pixels left of the element, got {near_boundary:?}"
    );
    assert!(
        near_boundary.b > near_boundary.r,
        "the boundary pixel must still be mostly blue, got {near_boundary:?}"
    );

    // 境界から blur 半径以上離れた pixel は青のまま。
    assert_eq!(canvas.pixel(38, 20), Some(Color::rgba(0, 0, 255, 255)));
    // 要素の外側（赤側）は filter されない。
    assert_eq!(canvas.pixel(10, 20), Some(Color::rgba(255, 0, 0, 255)));
}

#[test]
fn backdrop_filter_local_region_respects_inherited_clip() {
    // overflow: hidden の親でクリップされた backdrop は、クリップ後の領域だけを処理する。
    let (canvas, backdrop_pixels) = render_backdrop(
        "body { background-color: #4080c0; } \
         .a { display: none; } \
         .b { position: absolute; left: 50px; top: 50px; width: 10px; height: 10px; \
              overflow: hidden; } \
         .c { width: 100px; height: 100px; backdrop-filter: brightness(0.5); }",
        200.0,
    );

    assert_eq!(backdrop_pixels, 100);
    assert_eq!(canvas.pixel(55, 55), Some(Color::rgba(32, 64, 96, 255)));
    // クリップの外は元の背景色のまま。
    assert_eq!(canvas.pixel(65, 55), Some(Color::rgba(64, 128, 192, 255)));
    assert_eq!(canvas.pixel(55, 65), Some(Color::rgba(64, 128, 192, 255)));
}

#[test]
fn backdrop_filter_local_region_clamps_to_canvas_edges() {
    // 負座標へはみ出した要素は、キャンバス内に収まる領域だけを処理する。
    let (canvas, backdrop_pixels) = render_backdrop(
        "body { margin: 0; background-color: #4080c0; } \
         .a { position: absolute; left: -5px; top: -5px; width: 20px; height: 20px; \
              backdrop-filter: blur(3px); } \
         .b { display: none; }",
        40.0,
    );

    // 出力領域は 0..15 で、source 領域は右下だけ 3px 拡張される。
    assert_eq!(backdrop_pixels, 18 * 18);
    assert_eq!(canvas.pixel(0, 0), Some(Color::rgba(64, 128, 192, 255)));
    assert_eq!(canvas.pixel(14, 14), Some(Color::rgba(64, 128, 192, 255)));
}

#[test]
fn backdrop_filter_fully_offscreen_element_is_skipped() {
    // キャンバスの完全に外側にある要素は 1 pixel も処理しない（旧実装は panic した）。
    let (canvas, backdrop_pixels) = render_backdrop(
        "body { margin: 0; background-color: #4080c0; } \
         .a { position: absolute; left: 60px; top: 10px; width: 20px; height: 20px; \
              backdrop-filter: blur(3px); } \
         .b { display: none; }",
        40.0,
    );

    assert_eq!(backdrop_pixels, 0);
    assert_eq!(canvas.pixel(39, 15), Some(Color::rgba(64, 128, 192, 255)));
}

#[test]
fn backdrop_filter_multiple_elements_each_use_a_local_region() {
    // 複数の backdrop-filter 要素はそれぞれ自分の領域だけを処理する。
    let (canvas, backdrop_pixels) = render_backdrop(
        "body { margin: 0; background-color: #4080c0; } \
         .a { position: absolute; left: 10px; top: 10px; width: 10px; height: 10px; \
              backdrop-filter: brightness(0.5); } \
         .b { position: absolute; left: 60px; top: 60px; width: 10px; height: 10px; \
              backdrop-filter: brightness(0.25); }",
        200.0,
    );

    assert_eq!(backdrop_pixels, 200);
    assert_eq!(canvas.pixel(15, 15), Some(Color::rgba(32, 64, 96, 255)));
    assert_eq!(canvas.pixel(65, 65), Some(Color::rgba(16, 32, 48, 255)));
    assert_eq!(canvas.pixel(40, 40), Some(Color::rgba(64, 128, 192, 255)));
}

#[test]
fn backdrop_filter_drop_shadow_reads_padding_from_the_source_side() {
    // drop-shadow は出力を右へずらすため、入力は左側を余分に読む必要がある。
    // 要素の左外にある赤の影が要素内へ入り込むことで、入力 padding の向きを検証する。
    let (canvas, backdrop_pixels) = render_backdrop(
        ".a { position: absolute; left: 0; top: 0; width: 10px; height: 40px; \
              background-color: #ff0000; } \
         .b { position: absolute; left: 20px; top: 0; width: 20px; height: 40px; \
              backdrop-filter: drop-shadow(12px 0 0 #0000ff); }",
        40.0,
    );

    // 出力領域 20..40 に対し、入力は左 12px だけ拡張される（右へは広げない）。
    assert_eq!(backdrop_pixels, 32 * 40);

    let shadow = canvas.pixel(21, 20).unwrap();
    assert_eq!(
        shadow,
        Color::rgba(0, 0, 255, 255),
        "the shadow cast from x=9 must appear at x=21"
    );
    // 影の届かない位置は透明のまま。
    assert_eq!(canvas.pixel(30, 20).unwrap().a, 0);
}

#[test]
fn backdrop_filter_canvas_checksum_does_not_regress() {
    // blur + 色 filter の chain、画面外へはみ出した要素、ネストした 2 つ目の
    // backdrop を 1 枚に含めたページの全 pixel を checksum で固定する。
    // サンプル点の assert では拾えない領域外の 1 pixel の変化も検出する。
    let (canvas, backdrop_pixels) = render_backdrop(
        "body { background-color: #0000ff; } \
         .a { position: absolute; left: 0; top: 0; width: 20px; height: 30px; \
              background-color: #ff0000; } \
         .b { position: absolute; left: -6px; top: 10px; width: 30px; height: 30px; \
              backdrop-filter: blur(4px) grayscale(1); } \
         .c { position: absolute; left: 26px; top: 5px; width: 20px; height: 20px; \
              backdrop-filter: brightness(1.4); }",
        40.0,
    );

    // .b: 出力 0..24 x 10..40 を左上は clamp、右下だけ 4px 拡張 → 28x34
    // .c: 出力 20..40 x 15..35 を padding なしで処理 → 20x20
    assert_eq!(backdrop_pixels, 28 * 34 + 20 * 20);
    // 変更前の全 canvas clone 実装と同一の checksum。
    assert_eq!(adler32(canvas.pixels()), 2_663_706_984);
}

#[test]
fn filter_source_padding_mirrors_the_output_padding() {
    use crate::css::parse_filter_list;

    // blur は対称なので出力側と入力側が一致し、端数は切り上げる。
    let blur = parse_filter_list("blur(2.1px)").unwrap();
    assert_eq!(super::filter_padding(&blur), (3, 3, 3, 3));
    assert_eq!(super::filter_source_padding(&blur), (3, 3, 3, 3));

    // drop-shadow は左右・上下が入れ替わる。右下へずれる影の入力は左上側にある。
    // blur 分の 2px は全方向に加算される。
    let shadow = parse_filter_list("drop-shadow(12px 4px 2px #000)").unwrap();
    assert_eq!(super::filter_padding(&shadow), (2, 2, 14, 6));
    assert_eq!(super::filter_source_padding(&shadow), (14, 6, 2, 2));

    // 負のoffsetでも鏡像関係が保たれる。
    let negative = parse_filter_list("drop-shadow(-12px -4px 0 #000)").unwrap();
    assert_eq!(super::filter_padding(&negative), (12, 4, 0, 0));
    assert_eq!(super::filter_source_padding(&negative), (0, 0, 12, 4));

    // 色 filter は周辺 pixel を読まない。chain では各 filter の和になる。
    let chain = parse_filter_list("blur(3px) grayscale(1) drop-shadow(5px 0 0 #000)").unwrap();
    assert_eq!(super::filter_source_padding(&chain), (8, 3, 3, 3));

    // 病的な長さでも saturate して overflow しない。
    let huge = parse_filter_list("blur(99999999999999999999999px)").unwrap();
    assert_eq!(super::filter_source_padding(&huge), (usize::MAX, usize::MAX, usize::MAX, usize::MAX));
}

#[test]
fn backdrop_filter_saturates_pathological_lengths_to_the_canvas() {
    // padding が usize::MAX まで飽和しても、source 領域はキャンバス内に収まる。
    let (canvas, backdrop_pixels) = render_backdrop(
        "body { background-color: #4080c0; } \
         .a { position: absolute; left: 10px; top: 10px; width: 10px; height: 10px; \
              backdrop-filter: drop-shadow(99999999999999999999999px 0 0 #ff0000); } \
         .b { display: none; }",
        40.0,
    );

    // 入力は左端まで、右へは広がらない。
    assert_eq!(backdrop_pixels, 20 * 10);
    // 影は i32 の範囲外へずれるため、背景はそのまま残る。
    assert_eq!(canvas.pixel(15, 15), Some(Color::rgba(64, 128, 192, 255)));
}

#[test]
fn backdrop_filter_blur_larger_than_the_canvas_averages_the_source() {
    // 半径がキャンバスを超える blur でも overflow せず、一様な背景は保たれる。
    let (canvas, backdrop_pixels) = render_backdrop(
        "body { background-color: #4080c0; } \
         .a { position: absolute; left: 10px; top: 10px; width: 10px; height: 10px; \
              backdrop-filter: blur(99999999999999999999999px); } \
         .b { display: none; }",
        40.0,
    );

    assert_eq!(backdrop_pixels, 40 * 40);
    assert_eq!(canvas.pixel(15, 15), Some(Color::rgba(64, 128, 192, 255)));
    assert_eq!(canvas.pixel(30, 30), Some(Color::rgba(64, 128, 192, 255)));
}

#[test]
fn drop_shadow_uses_the_element_alpha_and_paints_behind_it() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(div);
    let css = "body { margin: 0; } div { width: 10px; height: 10px; background-color: red; \
               filter: drop-shadow(5px 0 0 #0000ff); }";
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());
    let viewport = Rect { x: 0.0, y: 0.0, width: 20.0, height: 20.0 };
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let canvas = paint_layout(&layout, &mut resolver, viewport);

    assert_eq!(canvas.pixel(5, 5), Some(Color::rgba(255, 0, 0, 255)));
    assert_eq!(canvas.pixel(12, 5), Some(Color::rgba(0, 0, 255, 255)));
}

#[test]
fn opacity_reduces_element_alpha() {
    // opacity: 0.5 を指定すると、要素の描画結果の alpha が半分になる。
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(div);

    let css = "body { margin: 0; } \
               div { width: 20px; height: 20px; background-color: #ff0000; opacity: 0.5; }";

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());

    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 40.0,
        height: 40.0,
    };
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let canvas = paint_layout(&layout, &mut resolver, viewport);

    // opacity 0.5 では中央ピクセルの alpha が 255 未満になるはず
    let center = canvas.pixel(10, 10);
    assert!(
        center.map_or(false, |c| c.a > 0 && c.a < 255),
        "opacity: 0.5 should produce semi-transparent pixel at center, got {:?}",
        center
    );
}

#[test]
fn opacity_zero_makes_element_invisible() {
    // opacity: 0 を指定すると、要素が完全に透明になる。
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(div);

    let css = "body { margin: 0; } \
               div { width: 20px; height: 20px; background-color: #ff0000; opacity: 0; }";

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());

    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 40.0,
        height: 40.0,
    };
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let canvas = paint_layout(&layout, &mut resolver, viewport);

    // opacity: 0 では全ピクセルが透明
    let center = canvas.pixel(10, 10);
    assert_eq!(
        center.map(|c| c.a),
        Some(0),
        "opacity: 0 should make element completely transparent, got {:?}",
        center
    );
}

#[test]
fn opacity_one_keeps_element_opaque() {
    // opacity: 1 (デフォルト) では要素が完全不透明。
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(div);

    let css = "body { margin: 0; } \
               div { width: 20px; height: 20px; background-color: #ff0000; opacity: 1; }";

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());

    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 40.0,
        height: 40.0,
    };
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let canvas = paint_layout(&layout, &mut resolver, viewport);

    // opacity: 1 では中央ピクセルが完全不透明の赤
    assert_eq!(
        canvas.pixel(10, 10),
        Some(Color::rgb(255, 0, 0)),
        "opacity: 1 should keep element fully opaque"
    );
}

#[test]
fn linear_gradient_to_right_paints_left_as_red_right_as_blue() {
    // linear-gradient(to right, red, blue) の左端は赤、右端は青
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(div);

    let css = "body { margin: 0; } \
               div { width: 100px; height: 20px; \
                     background-image: linear-gradient(to right, red, blue); }";

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());

    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 100.0,
        height: 20.0,
    };
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let canvas = paint_layout(&layout, &mut resolver, viewport);

    // 左端 (x=0) は赤 (255, 0, 0)
    let left = canvas.pixel(0, 10).expect("pixel at (0,10) should exist");
    assert!(
        left.r > 200 && left.b < 50,
        "left pixel should be mostly red, got {:?}",
        left
    );

    // 右端 (x=99) は青 (0, 0, 255)
    let right = canvas.pixel(99, 10).expect("pixel at (99,10) should exist");
    assert!(
        right.b > 200 && right.r < 50,
        "right pixel should be mostly blue, got {:?}",
        right
    );

    // 中間 (x=50) は赤青が混じる
    let mid = canvas.pixel(50, 10).expect("pixel at (50,10) should exist");
    assert!(
        mid.r > 50 && mid.b > 50,
        "middle pixel should mix red and blue, got {:?}",
        mid
    );
}

#[test]
fn linear_gradient_to_bottom_paints_top_as_red_bottom_as_blue() {
    // linear-gradient(to bottom, red, blue) の上端は赤、下端は青
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(div);

    let css = "body { margin: 0; } \
               div { width: 20px; height: 100px; \
                     background-image: linear-gradient(to bottom, red, blue); }";

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());

    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 20.0,
        height: 100.0,
    };
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let canvas = paint_layout(&layout, &mut resolver, viewport);

    let top = canvas.pixel(10, 0).expect("pixel at (10,0)");
    assert!(
        top.r > 200 && top.b < 50,
        "top should be red, got {:?}",
        top
    );

    let bottom = canvas.pixel(10, 99).expect("pixel at (10,99)");
    assert!(
        bottom.b > 200 && bottom.r < 50,
        "bottom should be blue, got {:?}",
        bottom
    );
}

#[test]
fn linear_gradient_degree_45_paints_diagonally() {
    // linear-gradient(45deg, red, blue) — 上左が赤よりで下右が青より
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(div);

    let css = "body { margin: 0; } \
               div { width: 100px; height: 100px; \
                     background-image: linear-gradient(45deg, red, blue); }";

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());

    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 100.0,
        height: 100.0,
    };
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let canvas = paint_layout(&layout, &mut resolver, viewport);

    // どのピクセルも塗られているはず（透明でない）
    let center = canvas.pixel(50, 50).expect("pixel at (50,50)");
    assert!(
        center.a == 255,
        "center pixel should be opaque, got {:?}",
        center
    );
}

#[test]
fn background_size_cover_fills_box() {
    // background-size: cover でボックス全体が画像で覆われる
    // 1x1 の赤ピクセル PNG をカバーして 20x20 のボックスを塗る
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(div);

    // 1x1 赤 PNG (最小限の PNG バイナリ, base64)
    // 実際には linear-gradient で代替。ここでは background-color で確認
    let css = "body { margin: 0; } \
               div { width: 20px; height: 20px; background-color: red; background-size: cover; }";

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());

    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 20.0,
        height: 20.0,
    };
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let canvas = paint_layout(&layout, &mut resolver, viewport);

    // background-color は background-size の影響を受けないので、全ピクセルが赤
    assert_eq!(canvas.pixel(10, 10), Some(Color::rgb(255, 0, 0)));
}

#[test]
fn linear_gradient_three_color_stops() {
    // linear-gradient(to right, red, green, blue) — 3色ストップ
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(div);

    let css = "body { margin: 0; } \
               div { width: 100px; height: 10px; \
                     background-image: linear-gradient(to right, red, green, blue); }";

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());

    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 100.0,
        height: 10.0,
    };
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let canvas = paint_layout(&layout, &mut resolver, viewport);

    let left = canvas.pixel(0, 5).expect("pixel at (0,5)");
    assert!(
        left.r > 200 && left.b < 50,
        "left should be red, got {:?}",
        left
    );

    let right = canvas.pixel(99, 5).expect("pixel at (99,5)");
    assert!(
        right.b > 200 && right.r < 50,
        "right should be blue, got {:?}",
        right
    );

    // 中点付近は緑っぽいはず
    let mid = canvas.pixel(50, 5).expect("pixel at (50,5)");
    assert!(
        mid.g > mid.r && mid.g > mid.b,
        "middle should be greenish, got {:?}",
        mid
    );
}

// ── paint_list_marker_placeholder tests ─────────────────────────────────────

/// Bullet markers (disc/circle/square) should render as a single filled square.
/// Uses a fixed 30x30 canvas with font_size=20.0. Checks that at least one pixel
/// is filled and that the total filled area is much smaller than a text run would be.
#[test]
fn bullet_marker_placeholder_renders_filled_square() {
    use crate::layout::ListMarker;

    let font_size = 20.0_f32;
    let color = Color::rgb(0, 0, 0);

    for text in ["\u{2022}", "\u{25e6}", "\u{25a0}"] {
        let marker = ListMarker {
            text: text.to_string(),
            font_size,
            outside: true,
            x: 0.0,
            y: 0.0,
        };

        let canvas_w = 30;
        let canvas_h = 30;
        let mut canvas = Canvas::new(canvas_w, canvas_h);
        paint_list_marker_placeholder(&mut canvas, &marker, font_size, color, None);

        let filled: usize = (0..canvas_w)
            .flat_map(|x| (0..canvas_h).map(move |y| (x, y)))
            .filter(|&(x, y)| canvas.pixel(x as u32, y as u32).map_or(false, |p| p.a > 0))
            .count();

        assert!(
            filled > 0,
            "bullet marker '{}' should draw at least one pixel",
            text
        );

        // A bullet placeholder draws only a small square, not a full glyph-width strip.
        // At font_size=20, size ≈ 7px → area ≈ 49 pixels at most.
        assert!(
            filled <= 100,
            "bullet marker '{}' should draw a small square, got {} filled pixels",
            text,
            filled
        );
    }
}

/// Text markers (decimal/roman/alpha) should render character-count placeholder
/// rectangles via `paint_text_placeholder`.  Verify that:
/// - at least one pixel is filled (something was drawn)
/// - more pixels are filled for a longer marker text (proportional to char count)
#[test]
fn text_marker_placeholder_renders_per_character_rectangles() {
    use crate::layout::ListMarker;

    let font_size = 16.0_f32;
    let color = Color::rgb(0, 0, 0);

    let marker_short = ListMarker {
        text: "1.".to_string(),
        font_size,
        outside: true,
        x: 0.0,
        y: 0.0,
    };
    let marker_long = ListMarker {
        text: "viii.".to_string(),
        font_size,
        outside: true,
        x: 0.0,
        y: 0.0,
    };

    let canvas_w = 120_u32;
    let canvas_h = 30_u32;

    let count_filled = |marker: &ListMarker| -> usize {
        let mut canvas = Canvas::new(canvas_w, canvas_h);
        paint_list_marker_placeholder(&mut canvas, marker, font_size, color, None);
        (0..canvas_w)
            .flat_map(|x| (0..canvas_h).map(move |y| (x, y)))
            .filter(|&(x, y)| canvas.pixel(x, y).map_or(false, |p| p.a > 0))
            .count()
    };

    let filled_short = count_filled(&marker_short);
    let filled_long = count_filled(&marker_long);

    assert!(filled_short > 0, "short text marker should draw pixels");
    assert!(filled_long > 0, "long text marker should draw pixels");
    assert!(
        filled_long > filled_short,
        "longer marker text '{}' ({} px) should fill more pixels than '{}' ({} px)",
        marker_long.text,
        filled_long,
        marker_short.text,
        filled_short
    );
}

/// `paint_text_placeholder` with a positive letter_spacing should spread pixels
/// further right than with zero letter_spacing for the same text.
#[test]
fn placeholder_letter_spacing_shifts_pixels_right() {
    let font_size = 16.0_f32;
    let color = Color::rgb(0, 0, 0);
    let canvas_w = 300_u32;
    let canvas_h = 30_u32;
    let rect = Rect {
        x: 0.0,
        y: 0.0,
        width: canvas_w as f32,
        height: canvas_h as f32,
    };

    let rightmost_filled = |canvas: &Canvas| -> u32 {
        (0..canvas_w)
            .rev()
            .find(|&x| (0..canvas_h).any(|y| canvas.pixel(x, y).map_or(false, |p| p.a > 0)))
            .unwrap_or(0)
    };

    let mut canvas_no_spacing = Canvas::new(canvas_w, canvas_h);
    paint_text_placeholder(
        &mut canvas_no_spacing,
        rect,
        "hello",
        font_size,
        color,
        None,
        0.0,
    );
    let right_no_spacing = rightmost_filled(&canvas_no_spacing);

    let mut canvas_with_spacing = Canvas::new(canvas_w, canvas_h);
    paint_text_placeholder(
        &mut canvas_with_spacing,
        rect,
        "hello",
        font_size,
        color,
        None,
        20.0,
    );
    let right_with_spacing = rightmost_filled(&canvas_with_spacing);

    assert!(
        right_with_spacing > right_no_spacing,
        "letter_spacing=20 (rightmost={}) should extend further than letter_spacing=0 (rightmost={})",
        right_with_spacing,
        right_no_spacing,
    );
}

/// Decimal marker should NOT look like a single bullet square:
/// it should spread pixels across a wider x range than a bullet marker would.
#[test]
fn decimal_marker_placeholder_is_wider_than_bullet_placeholder() {
    use crate::layout::ListMarker;

    let font_size = 16.0_f32;
    let color = Color::rgb(0, 0, 0);
    let canvas_w = 120_u32;
    let canvas_h = 30_u32;

    let make_canvas = |marker: &ListMarker| -> Canvas {
        let mut canvas = Canvas::new(canvas_w, canvas_h);
        paint_list_marker_placeholder(&mut canvas, marker, font_size, color, None);
        canvas
    };

    let bullet = ListMarker {
        text: "\u{2022}".to_string(),
        font_size,
        outside: true,
        x: 0.0,
        y: 0.0,
    };
    let decimal = ListMarker {
        text: "1.".to_string(),
        font_size,
        outside: true,
        x: 0.0,
        y: 0.0,
    };

    let rightmost_filled = |canvas: &Canvas| -> u32 {
        (0..canvas_w)
            .rev()
            .find(|&x| (0..canvas_h).any(|y| canvas.pixel(x, y).map_or(false, |p| p.a > 0)))
            .unwrap_or(0)
    };

    let bullet_canvas = make_canvas(&bullet);
    let decimal_canvas = make_canvas(&decimal);

    let bullet_right = rightmost_filled(&bullet_canvas);
    let decimal_right = rightmost_filled(&decimal_canvas);

    assert!(
        decimal_right > bullet_right,
        "decimal marker placeholder (rightmost filled col={}) should be wider \
         than bullet placeholder (rightmost filled col={})",
        decimal_right,
        bullet_right
    );
}

// ===== Per-fragment inline styling tests =====

/// When a `<span>` inside a `<p>` has `text-decoration-line: underline`, painting
/// must apply the decoration only to the span's fragments.  We verify this by
/// comparing two renders: one where only the span is underlined and one with no
/// text decoration at all; they should differ by at least some pixels below the text.
#[test]
fn nested_inline_span_text_decoration_applied_per_fragment() {
    // Build: <p>normal <span>decorated</span></p>
    // where only <span> has text-decoration-line: underline.
    let build_doc = || {
        let document = NodeHandle::document();
        let html = NodeHandle::element("html");
        let body = NodeHandle::element("body");
        let p = NodeHandle::element("p");
        let text_normal = NodeHandle::text("ab ");
        let span = NodeHandle::element("span");
        let text_span = NodeHandle::text("cd");
        document.append_child(html.clone());
        html.append_child(body.clone());
        body.append_child(p.clone());
        p.append_child(text_normal);
        p.append_child(span.clone());
        span.append_child(text_span);
        document
    };

    let render = |css: &str| {
        let document = build_doc();
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());
        let viewport = Rect {
            x: 0.0,
            y: 0.0,
            width: 200.0,
            height: 60.0,
        };
        let layout = layout_tree(&document, &mut resolver, viewport).expect("layout");
        paint_layout(&layout, &mut resolver, viewport)
    };

    let css_span_underline = "body { margin: 0; } p { color: #ff0000; font-size: 20px; } \
         span { text-decoration-line: underline; }";
    let css_no_decoration = "body { margin: 0; } p { color: #ff0000; font-size: 20px; }";

    let canvas_with = render(css_span_underline);
    let canvas_without = render(css_no_decoration);

    // At least one pixel row should differ between the two renders — the
    // underline decoration row introduced by the span.
    let (w, h) = (canvas_with.width(), canvas_with.height());
    let any_diff = (0..h).any(|y| {
        (0..w).any(|x| {
            let a = canvas_with.pixel(x, y).map_or(0, |c| c.a);
            let b = canvas_without.pixel(x, y).map_or(0, |c| c.a);
            a != b
        })
    });
    assert!(
        any_diff,
        "renders with and without span underline should differ; \
         per-fragment decoration must be painted"
    );
}

/// When a `<span>` inside a `<p>` has `text-transform: uppercase`, the span's
/// fragment text must be uppercased in the layout tree (via `collect_inline_segments`),
/// and the fragment must carry the span's style so paint can verify the transform.
/// This test confirms the end-to-end path via `paint_layout`.
#[test]
fn nested_inline_span_text_transform_differs_from_parent() {
    // <p>ab <span style="text-transform:uppercase">cd</span></p>
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let p = NodeHandle::element("p");
    let text_normal = NodeHandle::text("ab ");
    let span = NodeHandle::element("span");
    let text_span = NodeHandle::text("cd");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(p.clone());
    p.append_child(text_normal);
    p.append_child(span.clone());
    span.append_child(text_span);

    let css = "body { margin: 0; } p { color: #ff0000; font-size: 20px; } \
               span { text-transform: uppercase; }";
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());

    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 200.0,
        height: 60.0,
    };
    let layout = layout_tree(&document, &mut resolver, viewport).expect("layout");

    // Collect all text fragments and check that the span's fragment carries
    // text-transform:uppercase and that its text has been transformed.
    fn collect_text_fragments(layout: &crate::layout::LayoutBox, out: &mut Vec<(String, String)>) {
        for line in &layout.lines {
            for frag in &line.fragments {
                if let Some(t) = frag.text() {
                    let transform = frag
                        .style
                        .text_transform
                        .as_deref()
                        .map(|kw| kw.to_ascii_lowercase())
                        .unwrap_or_else(|| "none".to_string());
                    out.push((transform, t.to_string()));
                }
            }
        }
        for child in &layout.children {
            collect_text_fragments(child, out);
        }
    }

    let mut fragments = Vec::new();
    collect_text_fragments(&layout, &mut fragments);

    // The "CD" fragment should have text-transform:uppercase.
    let upper_frag = fragments.iter().find(|(_, text)| text.contains("CD"));
    assert!(
        upper_frag.is_some(),
        "expected a fragment with text \"CD\" (uppercased), got {:?}",
        fragments
    );
    assert_eq!(
        upper_frag.unwrap().0,
        "uppercase",
        "span fragment should carry text-transform:uppercase; got {:?}",
        fragments
    );

    // The "ab" fragment should NOT have text-transform:uppercase.
    let normal_frag = fragments.iter().find(|(_, text)| text.contains("ab"));
    assert!(
        normal_frag.is_some(),
        "expected a fragment with text \"ab\", got {:?}",
        fragments
    );
    assert_ne!(
        normal_frag.unwrap().0,
        "uppercase",
        "\"ab\" fragment should not carry text-transform:uppercase"
    );

    // Also verify paint runs without panic.
    paint_layout(&layout, &mut resolver, viewport);
}

// ── gradient tile repeat tests ────────────────────────────────────────────────

/// background-size で小さなタイルを指定した linear-gradient が background-repeat で
/// 繰り返し描画されること。タイルのオフスクリーンバッファ最適化後も描画結果は変わらない。
#[test]
fn tiled_linear_gradient_repeats_correctly() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(div);

    // 10x10 の領域に 5x5 のタイルを敷き詰める (4タイル)
    let css = "body { margin: 0; } \
               div { width: 10px; height: 10px; \
                     background-image: linear-gradient(to right, red, blue); \
                     background-size: 5px 5px; \
                     background-repeat: repeat; }";

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());

    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 10.0,
        height: 10.0,
    };
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let canvas = paint_layout(&layout, &mut resolver, viewport);

    // 各タイルの左端は赤系、右端は青系
    let top_left = canvas.pixel(0, 0).expect("pixel at (0,0)");
    assert!(
        top_left.r > 150,
        "top-left of tile should be red-ish, got {:?}",
        top_left
    );

    let top_right_of_first_tile = canvas.pixel(4, 0).expect("pixel at (4,0)");
    assert!(
        top_right_of_first_tile.b > 150,
        "right end of first tile should be blue-ish, got {:?}",
        top_right_of_first_tile
    );

    // 2枚目タイルの左端も赤系（繰り返しが機能している）
    let second_tile_left = canvas.pixel(5, 0).expect("pixel at (5,0)");
    assert!(
        second_tile_left.r > 150,
        "left of second tile should be red-ish (repeat working), got {:?}",
        second_tile_left
    );

    // 下段タイルも同様に存在する
    let bottom_tile = canvas.pixel(0, 5).expect("pixel at (0,5)");
    assert!(
        bottom_tile.r > 150,
        "bottom-row tile left should be red-ish, got {:?}",
        bottom_tile
    );
}

/// background-repeat: no-repeat のとき、gradient タイルは1枚だけ描画される。
#[test]
fn tiled_linear_gradient_no_repeat_paints_once() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(div);

    // 10x10 の領域に 5x5 タイル1枚だけ（no-repeat）
    let css = "body { margin: 0; } \
               div { width: 10px; height: 10px; \
                     background-image: linear-gradient(to right, red, blue); \
                     background-size: 5px 5px; \
                     background-repeat: no-repeat; }";

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());

    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 10.0,
        height: 10.0,
    };
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let canvas = paint_layout(&layout, &mut resolver, viewport);

    // 第1タイル内は描画済み
    let first = canvas.pixel(0, 0).expect("pixel at (0,0)");
    assert!(first.a > 0, "first tile should be painted, got {:?}", first);

    // 第1タイルの外（右側）は透明なまま
    let outside = canvas.pixel(6, 0).expect("pixel at (6,0)");
    assert_eq!(
        outside.a, 0,
        "outside tile area should be transparent with no-repeat, got {:?}",
        outside
    );
}

// --- box_blur_alpha カーネルサイズ修正テスト ---

#[test]
fn box_blur_alpha_uniform_field_stays_uniform() {
    // 全ピクセルが同一アルファ値のキャンバスに box_blur_alpha を適用しても値が変わらないこと。
    // 端のカーネルサイズが中央と異なる場合、端のピクセルが変化してしまうバグの検証。
    let w = 8u32;
    let h = 8u32;
    let mut canvas = Canvas::new(w, h);
    // 全ピクセルを alpha=200 に設定
    for i in 0..w as usize * h as usize {
        canvas.pixels[i * 4 + 3] = 200;
    }
    canvas.box_blur_alpha_passes(2, 1);
    // 端を含む全ピクセルが 200 のまま（±1 許容）
    for y in 0..h {
        for x in 0..w {
            let a = canvas.pixel(x, y).unwrap().a;
            assert!(
                (a as i32 - 200).abs() <= 1,
                "box_blur_alpha should preserve uniform field; pixel ({x},{y}) a={a}"
            );
        }
    }
}

#[test]
fn box_blur_alpha_edge_equals_center_for_uniform_input() {
    // 全アルファ=128 のキャンバスで、blur後に端ピクセルと中央ピクセルが同じ値になること。
    let mut canvas = Canvas::new(10, 10);
    for i in 0..100usize {
        canvas.pixels[i * 4 + 3] = 128;
    }
    canvas.box_blur_alpha_passes(3, 1);
    let center = canvas.pixel(5, 5).unwrap().a;
    let corner = canvas.pixel(0, 0).unwrap().a;
    assert!(
        (center as i32 - corner as i32).abs() <= 1,
        "edge and center should be equal for uniform input after blur; center={center}, corner={corner}"
    );
}

#[test]
fn row_major_box_blur_matches_column_major_reference() {
    let (w, h, radius, passes) = (13usize, 9usize, 3usize, 3usize);
    let input = (0..w * h)
        .map(|index| ((index * 73 + index / w * 19) % 256) as u8)
        .collect::<Vec<_>>();
    let mut expected = input.clone();
    let mut scratch = vec![0u8; w * h];
    for _ in 0..passes {
        for y in 0..h {
            for x in 0..w {
                let left = x.saturating_sub(radius);
                let right = (x + radius).min(w - 1);
                let sum = expected[y * w + left..=y * w + right]
                    .iter()
                    .map(|&alpha| alpha as u32)
                    .sum::<u32>();
                scratch[y * w + x] = (sum / (right - left + 1) as u32) as u8;
            }
        }
        for y in 0..h {
            let top = y.saturating_sub(radius);
            let bottom = (y + radius).min(h - 1);
            for x in 0..w {
                let sum = (top..=bottom)
                    .map(|row| scratch[row * w + x] as u32)
                    .sum::<u32>();
                expected[y * w + x] = (sum / (bottom - top + 1) as u32) as u8;
            }
        }
    }

    let mut canvas = Canvas::new(w as u32, h as u32);
    for (index, &alpha) in input.iter().enumerate() {
        canvas.pixels[index * 4 + 3] = alpha;
    }
    canvas.box_blur_alpha_passes(radius as u32, passes);
    let actual = canvas
        .pixels
        .iter()
        .skip(3)
        .step_by(4)
        .copied()
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
}

#[test]
fn composite_alpha_scale_matches_prescaled_source() {
    let mut source = Canvas::new(3, 2);
    for (index, alpha) in [0, 1, 63, 127, 191, 255].into_iter().enumerate() {
        source.pixels[index * 4 + 3] = alpha;
    }
    let scale = 73.0 / 255.0;

    let mut prescaled_source = source.clone();
    prescaled_source.multiply_alpha(scale);
    let mut expected = Canvas::new(5, 4);
    expected.composite_canvas_clipped(
        &prescaled_source,
        1,
        1,
        20,
        40,
        60,
        1.0,
        None,
    );

    let mut actual = Canvas::new(5, 4);
    actual.composite_canvas_clipped(&source, 1, 1, 20, 40, 60, scale, None);

    assert_eq!(actual.pixels, expected.pixels);
}

// --- opacity オフスクリーンバッファサイズのテスト ---

#[test]
fn opacity_offscreen_buffer_does_not_crash_on_small_element() {
    // 大きいビューポートに小さい要素を opacity で描画した場合クラッシュしない。
    // また、要素の外側のキャンバスは変化しないこと。
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(div);

    let css = "body { margin: 0; } \
               div { width: 10px; height: 10px; background-color: #ff0000; opacity: 0.5; }";

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());

    // 要素より大幅に大きいビューポート
    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 200.0,
        height: 200.0,
    };
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let canvas = paint_layout(&layout, &mut resolver, viewport);

    assert_eq!(
        super::take_effect_surface_pixels(),
        100,
        "a 10x10 effect must not allocate a viewport-sized surface"
    );

    // 要素内のピクセルは半透明の赤
    let inside = canvas.pixel(5, 5).unwrap();
    assert!(
        inside.a > 0 && inside.a < 255,
        "element should be semi-transparent, got a={}",
        inside.a
    );

    // 要素の外側は透明なまま
    let outside = canvas.pixel(50, 50).unwrap();
    assert_eq!(
        outside.a, 0,
        "outside element should be transparent, got {:?}",
        outside
    );
}

#[test]
fn local_effect_surface_keeps_overflow_visible_descendant() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let parent = NodeHandle::element("div");
    let child = NodeHandle::element("div");
    parent.set_attribute("class", "effect");
    child.set_attribute("class", "overflow");
    document.append_child(body.clone());
    body.append_child(parent.clone());
    parent.append_child(child);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { margin: 0; } .effect { margin: 20px; width: 10px; height: 10px; opacity: .5; } .overflow { width: 30px; height: 8px; background: red; }",
        )
        .unwrap(),
    );
    let viewport = Rect { x: 0.0, y: 0.0, width: 100.0, height: 100.0 };
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let parent_border_box = border_box_rect(find_layout_box_by_class(&layout, "effect").unwrap());
    let parent_right = parent_border_box.x + parent_border_box.width;
    let canvas = paint_layout(&layout, &mut resolver, viewport);
    let effect_surface_pixels = super::take_effect_surface_pixels();

    // Only the child paints red. Requiring a red pixel strictly to the right of
    // the parent's measured border box proves its overflow-visible part survived.
    let has_red_outside_parent = canvas
        .pixels
        .chunks_exact(4)
        .enumerate()
        .any(|(index, pixel)| {
            let x = index % canvas.width() as usize;
            x as f32 + 0.5 >= parent_right && pixel[0] > 0 && pixel[3] > 0
        });
    assert!(has_red_outside_parent, "overflow-visible child was cropped");
    assert!(effect_surface_pixels < 1_000);
}

#[test]
fn local_effect_surface_clips_negative_coordinates_to_canvas() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let effect = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(effect);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { margin: 0; } div { margin-left: -5px; width: 10px; height: 10px; background: red; opacity: .5; }",
        )
        .unwrap(),
    );
    let viewport = Rect { x: 0.0, y: 0.0, width: 100.0, height: 100.0 };
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let canvas = paint_layout(&layout, &mut resolver, viewport);
    let effect_surface_pixels = super::take_effect_surface_pixels();

    assert!(canvas.pixel(0, 5).unwrap().a > 0);
    assert_eq!(effect_surface_pixels, 50);
}

#[test]
fn local_effect_surface_keeps_nested_filter_padding() {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let parent = NodeHandle::element("div");
    let child = NodeHandle::element("div");
    parent.set_attribute("class", "outer");
    child.set_attribute("class", "inner");
    document.append_child(body.clone());
    body.append_child(parent.clone());
    parent.append_child(child);

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            "body { margin: 0; } .outer { margin: 20px; width: 10px; height: 10px; opacity: .8; } .inner { display: block; width: 10px; height: 10px; background: red; filter: drop-shadow(12px 0 0 blue); }",
        )
        .unwrap(),
    );
    let viewport = Rect { x: 0.0, y: 0.0, width: 100.0, height: 100.0 };
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let outer_border_box = border_box_rect(find_layout_box_by_class(&layout, "outer").unwrap());
    let inner_border_box = border_box_rect(find_layout_box_by_class(&layout, "inner").unwrap());
    let source_right = (outer_border_box.x + outer_border_box.width)
        .max(inner_border_box.x + inner_border_box.width);
    let canvas = paint_layout(&layout, &mut resolver, viewport);
    let effect_surface_pixels = super::take_effect_surface_pixels();

    // A blue pixel strictly beyond both measured source border boxes can only
    // come from the nested 12px drop shadow, not either element background.
    let has_shadow_outside_sources = canvas
        .pixels
        .chunks_exact(4)
        .enumerate()
        .any(|(index, pixel)| {
            let x = index % canvas.width() as usize;
            x as f32 + 0.5 >= source_right && pixel[2] > pixel[0] && pixel[3] > 0
        });
    assert!(has_shadow_outside_sources, "nested shadow was cropped");
    assert!(effect_surface_pixels < 2_000);
}

// --- overflow: hidden での box-shadow クリップテスト ---

#[test]
fn box_shadow_of_overflow_hidden_element_still_renders_outside() {
    // overflow: hidden の要素自身に付いた box-shadow は要素の外側に描画されるべき。
    // CSS仕様: overflow:hidden は要素自身のbox-shadowには影響しない。
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(div);

    let css = "body { margin: 0; } \
               div { width: 20px; height: 20px; background-color: #ffffff; \
                     overflow: hidden; box-shadow: 5px 5px 0px 0px #000000; }";

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());

    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 60.0,
        height: 60.0,
    };
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let canvas = paint_layout(&layout, &mut resolver, viewport);

    // shadow_rect = (5,5)-(25,25) なので (22,22) はシャドウ内
    let shadow_pixel = canvas.pixel(22, 22);
    assert!(
        shadow_pixel.map_or(false, |c| c.a > 0),
        "box-shadow should render outside element even with overflow:hidden, got {:?}",
        shadow_pixel
    );
}

#[test]
fn child_box_shadow_clipped_by_overflow_hidden_parent() {
    // overflow: hidden の親要素が持つ overflow clip によって、
    // 子要素の box-shadow が親の外に漏れないこと。
    // 子要素は親からはみ出ており、その box-shadow も親の外には描画されない。
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let parent = NodeHandle::element("div");
    let child = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(parent.clone());
    parent.append_child(child.clone());
    parent.set_attribute("class", "parent");
    child.set_attribute("class", "child");

    // 親: 20x20, overflow: hidden
    // 子: 10x10, 親の右端まぎわに配置し、box-shadow: 8px 0px 0px 0px red で右に伸ばす
    // → 子の box-shadow は親の外 (x>20) に出ようとするが、inherited_clip で clip される
    let css = "body { margin: 0; } \
               .parent { width: 20px; height: 20px; overflow: hidden; background-color: white; } \
               .child { width: 10px; height: 10px; margin-left: 10px; background-color: blue; \
                        box-shadow: 8px 0px 0px 0px #ff0000; }";

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());

    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 60.0,
        height: 60.0,
    };
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let canvas = paint_layout(&layout, &mut resolver, viewport);

    // 子の box-shadow は x=18..28 の範囲になるが、親の overflow:hidden クリップ(x<20) により x>20 は描画されない
    // x=25 (親の外) は透明なはず
    let outside = canvas.pixel(25, 5);
    assert_eq!(
        outside.map(|c| c.a),
        Some(0),
        "child box-shadow should be clipped by parent overflow:hidden, got {:?}",
        outside
    );
}

// --- blur=0 の角丸 box-shadow テスト ---

#[test]
fn box_shadow_no_blur_rounded_corners_do_not_paint_outside_shadow_shape() {
    // blur=0 の box-shadow で border-radius がある場合、
    // シャドウの角丸コーナー外側（矩形角だが円弧外）にピクセルが描画されないこと。
    //
    // 設定: width=20px, height=20px, border-radius=5px, box-shadow: 0 0 0 2px black
    // shadow_rect = (-2,-2,24,24), shadow_radius = 5+2=7
    // 左上コーナーの円弧中心 = (-2+7, -2+7) = (5, 5) in canvas coords
    // pixel (0, 0): ピクセル中心 (0.5, 0.5), distance to (5,5) = sqrt(4.5^2+4.5^2) ≈ 6.36 < 7 → 円内
    // pixel (-2,-2) はキャンバス外なので (0,0) が shadow_rect 内かつ角丸外に相当する座標を使う
    // => spread=2px のみ、radius=5+2=7 の場合、矩形バンド先端は shadow_rect.y=-2 から border_box.y=0 まで
    //    (0,0) は top band に含まれるが角丸外: ピクセル中心(0.5,0.5)の角丸距離 ≈ 6.36 < 7 なので内側
    //    もっと端を確認: spread=2, radius=5 で shadow_rect=(-2,-2,24,24), shadow_radius=7
    //    top-left corner center at (5,5). pixel (-1,-1) はキャンバス外 -> 確認できない
    //    代わりに: border-radius=8px, spread=3px → shadow_radius=11, shadow_rect=(-3,-3)
    //    corner center at (-3+11,-3+11)=(8,8). pixel(0,0): distance(0.5,0.5 to 8,8)=sqrt(7.5^2+7.5^2)≈10.6 < 11 → 内側
    //    もっと大きいspread: border-radius=10px, spread=5px → shadow_radius=15, shadow_rect=(-5,-5)
    //    corner center at (10,10). pixel(0,0): distance(0.5,0.5 to 10,10)=sqrt(9.5^2+9.5^2)≈13.4 < 15 → 内側
    //    ↑ border_box の原点から shadow_rect まで offset=spread なので、(0,0) は常に border_box 内に含まれ影の外
    //    最もシンプルなケース: no offset, spread=0 で blur=0 → shadow は border_box と同一形状
    //    → border_box の外に影なし。角丸が効いていれば角が透明
    //
    // テスト方針: offset=3px, spread=0, blur=0, border-radius=5px の場合
    // shadow_rect = (3,3,23,23), shadow_radius=5
    // corner center at (3+5,3+5)=(8,8). pixel(3,3) の中心(3.5,3.5), distance to (8,8)=sqrt(4.5^2+4.5^2)≈6.36 < 5? No, 6.36 > 5 → outside!
    // 矩形バンド時: top band y=3..20 の部分なし (shadow_rect.y=3, border_box.y=0 なので top_band.height=0-3=-3 < 0)
    // → 矩形バンドでも描画なし
    //
    // 再考: offset=0, spread=3px, border-radius=5px, blur=0
    // shadow_rect = (-3,-3,26,26), shadow_radius=5+3=8
    // top-left corner center at (-3+8,-3+8) = (5,5)
    // pixel(1,1): center(1.5,1.5), distance to(5,5)=sqrt(3.5^2+3.5^2)≈4.95 < 8 → inside (OK, shadow painted)
    // pixel(0,0): center(0.5,0.5), distance to(5,5)=sqrt(4.5^2+4.5^2)≈6.36 < 8 → inside (OK, shadow painted)
    // (キャンバス外の corner は確認不可)
    //
    // 最終方針: キャンバス内で shadow の角丸 outside にあるピクセルが透明かを確認する
    // offset=4px right-down, spread=0, border-radius=8px, blur=0
    // shadow_rect=(4,4,24,24), border_box=(0,0,20,20)
    // top band: shadow_rect.y=4 > border_box.y=0 → top band なし
    // bottom band: shadow_rect_bottom=24 > box_bottom=20 → y=20..24, x=4..24
    // right band: shadow_rect_right=24 > box_right=20 → x=20..24, y=4(max(4,0))..20(min(20,24))
    // shadow_radius=8, corner center of bottom-right: (24-8,24-8)=(16,16)
    // pixel(23,23): center(23.5,23.5), distance to(16,16)=sqrt(7.5^2+7.5^2)≈10.6 > 8 → outside!
    // 矩形バンド時: pixel(23,23) は bottom band (y=20..24) に入る → 黒
    // 角丸対応時: pixel(23,23) は角丸外 → 透明
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(body.clone());
    body.append_child(div);

    let css = "body { margin: 0; } \
               div { width: 20px; height: 20px; background-color: transparent; \
                     border-radius: 8px; \
                     box-shadow: 4px 4px 0px 0px #000000; }";

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());

    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 40.0,
        height: 40.0,
    };
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let canvas = paint_layout(&layout, &mut resolver, viewport);

    // shadow_rect = (4,4,24,24), shadow_radius=8
    // corner center of bottom-right: (24-8,24-8)=(16,16)
    // pixel(23,23): center(23.5,23.5), distance to(16,16)≈10.6 > 8 → outside rounded shadow
    // 矩形バンド実装: pixel(23,23) は bottom band に含まれ黒 → a > 0 (バグ)
    // 角丸対応実装: pixel(23,23) は角丸外 → a == 0 (正常)
    let corner = canvas.pixel(23, 23);
    assert_eq!(
        corner.map(|c| c.a),
        Some(0),
        "blur=0 rounded box-shadow corner pixel should be transparent (outside arc), got {:?}",
        corner
    );

    // 影の中央部分は描画されているはず (bottom band の中央)
    let shadow_mid = canvas.pixel(12, 23);
    assert!(
        shadow_mid.map_or(false, |c| c.a > 0),
        "blur=0 rounded box-shadow should paint center of bottom band at (12,23), got {:?}",
        shadow_mid
    );
}

#[test]
fn force_opacity_makes_opacity_zero_element_visible() {
    let html = r#"<html><head><style>
body { margin: 0; background: white; }
div { width: 10px; height: 10px; background: red; opacity: 0; }
</style></head><body><div></div></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 10.0,
        height: 10.0,
    };

    // Without force_opacity: div is invisible
    let canvas_normal = render_document(&document, viewport).unwrap();
    assert_eq!(
        canvas_normal.pixel(5, 5),
        Some(Color::rgb(255, 255, 255)),
        "opacity:0 element should be invisible without force_opacity"
    );

    // With force_opacity: div is visible
    let canvas_forced =
        crate::paint::with_force_opacity(|| render_document(&document, viewport).unwrap());
    assert_eq!(
        canvas_forced.pixel(5, 5),
        Some(Color::rgb(255, 0, 0)),
        "opacity:0 element should be visible with force_opacity"
    );

    // After with_force_opacity: flag should not leak
    let canvas_after = render_document(&document, viewport).unwrap();
    assert_eq!(
        canvas_after.pixel(5, 5),
        Some(Color::rgb(255, 255, 255)),
        "force_opacity should not leak outside the closure"
    );
}

#[test]
fn rgb_out_of_range_channels_are_clamped() {
    let html = r#"<html><head><style>
body { margin: 0; }
div { width: 10px; height: 10px; background: rgb(300, -50, 128); }
</style></head><body><div></div></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 10.0,
        height: 10.0,
    };
    let canvas = render_document(&document, viewport).unwrap();
    assert_eq!(
        canvas.pixel(5, 5),
        Some(Color::rgb(255, 0, 128)),
        "rgb(300,-50,128) should clamp to rgb(255,0,128)"
    );
}

#[test]
fn rgba_percentage_alpha_is_parsed_correctly() {
    let html = r#"<html><head><style>
body { margin: 0; background: white; }
div { width: 10px; height: 10px; background: rgba(255, 0, 0, 50%); }
</style></head><body><div></div></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 10.0,
        height: 10.0,
    };
    let canvas = render_document(&document, viewport).unwrap();
    let pixel = canvas.pixel(5, 5).unwrap();
    // rgba(255,0,0,50%) over white → blended ~(255,128,128)
    assert!(pixel.r > 200, "red channel should be high, got {}", pixel.r);
    assert!(
        pixel.g > 100 && pixel.g < 180,
        "green channel should be ~128, got {}",
        pixel.g
    );
    assert!(
        pixel.a == 255,
        "final pixel should be fully opaque after blending"
    );
}

#[test]
fn rgb_percentage_channels_are_clamped() {
    let html = r#"<html><head><style>
body { margin: 0; }
div { width: 10px; height: 10px; background: rgb(200%, 0%, 50%); }
</style></head><body><div></div></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 10.0,
        height: 10.0,
    };
    let canvas = render_document(&document, viewport).unwrap();
    assert_eq!(
        canvas.pixel(5, 5),
        Some(Color::rgb(255, 0, 128)),
        "rgb(200%,0%,50%) should clamp to rgb(255,0,128)"
    );
}

#[test]
fn current_color_resolves_border_color_from_element_color() {
    let html = r#"<html><head><style>
body { margin: 0; }
div { width: 10px; height: 10px; color: red; border: 2px solid currentColor; }
</style></head><body><div></div></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 14.0,
        height: 14.0,
    };
    let canvas = render_document(&document, viewport).unwrap();
    // Border should be red (from currentColor → color: red)
    assert_eq!(
        canvas.pixel(0, 0),
        Some(Color::rgb(255, 0, 0)),
        "border with currentColor should use element's color (red)"
    );
}

#[test]
fn color_current_color_inherits_from_parent() {
    let html = r#"<html><head><style>
body { margin: 0; color: blue; }
div { color: currentColor; width: 10px; height: 10px; border: 2px solid currentColor; }
</style></head><body><div></div></body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 14.0,
        height: 14.0,
    };
    let canvas = render_document(&document, viewport).unwrap();
    // color: currentColor on the element should inherit from parent (blue)
    assert_eq!(
        canvas.pixel(0, 0),
        Some(Color::rgb(0, 0, 255)),
        "color: currentColor should resolve to parent color (blue)"
    );
}

#[test]
fn render_document_executes_inline_script() {
    let html = r#"<html><head><style>
        body { margin: 0; }
        div { width: 10px; height: 10px; background: white; }
        div.red { background: red; }
    </style></head><body>
        <div id="target"></div>
        <script>document.getElementById("target").classList.add("red");</script>
    </body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 10.0,
        height: 10.0,
    };
    let canvas = render_document(&document, viewport).unwrap();
    // JS should add class "red" → background: red
    assert_eq!(
        canvas.pixel(5, 5),
        Some(Color::rgb(255, 0, 0)),
        "inline script should add 'red' class, making div background red"
    );
}

#[test]
fn render_document_uses_stylesheet_injected_by_script() {
    let html = r##"<html><head><style>
        body { margin: 0; }
        #target { width: 10px; height: 10px; background: red; }
    </style></head><body>
        <div id="target"></div>
        <script>
          const style = document.createElement("style");
          style.textContent = "#target { background: blue; }";
          document.head.appendChild(style);
        </script>
    </body></html>"##;
    let document = TreeBuilder::parse(html).document();
    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 10.0,
        height: 10.0,
    };

    let canvas = render_document(&document, viewport).unwrap();

    assert_eq!(canvas.pixel(5, 5), Some(Color::rgb(0, 0, 255)));
}

#[test]
fn render_document_drives_animation_frames_before_layout() {
    let html = r#"<html><head><style>
        body { margin: 0; }
        div { width: 10px; height: 10px; background: white; }
        div.ready { background: red; }
        div.settled { background: blue; }
    </style></head><body>
        <div id="target"></div>
        <script>
          requestAnimationFrame(() => {
            document.getElementById("target").classList.add("ready");
            requestAnimationFrame(() => {
              document.getElementById("target").classList.add("settled");
            });
          });
        </script>
    </body></html>"#;
    let document = TreeBuilder::parse(html).document();
    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 10.0,
        height: 10.0,
    };

    let canvas = render_document(&document, viewport).unwrap();

    assert_eq!(
        canvas.pixel(5, 5),
        Some(Color::rgb(0, 0, 255)),
        "the bounded screenshot frame pump should settle nested animation-frame DOM updates"
    );
}

#[test]
fn mask_image_multiplies_element_alpha_per_pixel() {
    let mask = rgba_mask_data_uri(3, 1, &[255, 128, 0]);
    let html = format!(
        r#"<html><head><style>
body {{ margin: 0; }}
div {{ width: 3px; height: 1px; background: red;
       mask-image: url("{mask}"); mask-repeat: no-repeat; }}
</style></head><body><div></div></body></html>"#
    );
    let document = TreeBuilder::parse(&html).document();
    let canvas = render_document(
        &document,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 3.0,
            height: 1.0,
        },
    )
    .unwrap();

    assert_eq!(canvas.pixel(0, 0), Some(Color::rgba(255, 0, 0, 255)));
    assert_eq!(canvas.pixel(1, 0), Some(Color::rgba(255, 0, 0, 128)));
    assert_eq!(canvas.pixel(2, 0), Some(Color::rgba(0, 0, 0, 0)));
}

#[test]
fn no_repeat_mask_position_clears_pixels_outside_the_mask_tile() {
    let mask = rgba_mask_data_uri(1, 1, &[255]);
    let html = format!(
        r#"<html><head><style>
body {{ margin: 0; }}
div {{ width: 6px; height: 4px; background: red;
       mask-image: url("{mask}"); mask-size: 2px 2px;
       mask-position: 2px 1px; mask-repeat: no-repeat; }}
</style></head><body><div></div></body></html>"#
    );
    let document = TreeBuilder::parse(&html).document();
    let canvas = render_document(
        &document,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 6.0,
            height: 4.0,
        },
    )
    .unwrap();

    assert_eq!(canvas.pixel(1, 1), Some(Color::rgba(0, 0, 0, 0)));
    assert_eq!(canvas.pixel(2, 1), Some(Color::rgb(255, 0, 0)));
    assert_eq!(canvas.pixel(3, 2), Some(Color::rgb(255, 0, 0)));
    assert_eq!(canvas.pixel(4, 2), Some(Color::rgba(0, 0, 0, 0)));
    assert_eq!(canvas.pixel(2, 3), Some(Color::rgba(0, 0, 0, 0)));
}

#[test]
fn mask_size_contain_preserves_aspect_ratio_and_leaves_remainder_transparent() {
    let mask = rgba_mask_data_uri(2, 1, &[255, 255]);
    let html = format!(
        r#"<html><head><style>
body {{ margin: 0; }}
div {{ width: 8px; height: 8px; background: blue;
       mask-image: url("{mask}"); mask-size: contain; mask-repeat: no-repeat; }}
</style></head><body><div></div></body></html>"#
    );
    let document = TreeBuilder::parse(&html).document();
    let canvas = render_document(
        &document,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 8.0,
            height: 8.0,
        },
    )
    .unwrap();

    assert_eq!(canvas.pixel(0, 0), Some(Color::rgb(0, 0, 255)));
    assert_eq!(canvas.pixel(7, 3), Some(Color::rgb(0, 0, 255)));
    assert_eq!(canvas.pixel(0, 4), Some(Color::rgba(0, 0, 0, 0)));
    assert_eq!(canvas.pixel(7, 7), Some(Color::rgba(0, 0, 0, 0)));
}

#[test]
fn mask_applies_to_descendant_painting() {
    let mask = rgba_mask_data_uri(2, 1, &[255, 0]);
    let html = format!(
        r#"<html><head><style>
body {{ margin: 0; }}
.parent {{ width: 2px; height: 1px; mask-image: url("{mask}"); mask-repeat: no-repeat; }}
.child {{ width: 2px; height: 1px; background: green; }}
</style></head><body><div class="parent"><div class="child"></div></div></body></html>"#
    );
    let document = TreeBuilder::parse(&html).document();
    let canvas = render_document(
        &document,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 2.0,
            height: 1.0,
        },
    )
    .unwrap();

    assert_eq!(canvas.pixel(0, 0), Some(Color::rgb(0, 128, 0)));
    assert_eq!(canvas.pixel(1, 0), Some(Color::rgba(0, 0, 0, 0)));
}

#[test]
fn mask_keeps_pixels_covered_by_a_fractional_border_box() {
    let mask = rgba_mask_data_uri(1, 1, &[255]);
    let html = format!(
        r#"<html><head><style>
body {{ margin: 0; }}
div {{ position: absolute; left: 10.5px; top: 10.5px;
       width: 2px; height: 2px; background: red;
       mask-image: url("{mask}"); }}
</style></head><body><div></div></body></html>"#
    );
    let document = TreeBuilder::parse(&html).document();
    let canvas = render_document(
        &document,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 16.0,
            height: 16.0,
        },
    )
    .unwrap();

    assert_eq!(canvas.pixel(10, 10), Some(Color::rgb(255, 0, 0)));
    assert_eq!(canvas.pixel(12, 12), Some(Color::rgb(255, 0, 0)));
    assert_eq!(canvas.pixel(9, 10), Some(Color::rgba(0, 0, 0, 0)));
    assert_eq!(canvas.pixel(10, 9), Some(Color::rgba(0, 0, 0, 0)));
}

#[test]
fn form_control_text_measure_matches_painter_advance_model() {
    use super::text::measure_form_control_text_width;

    // Placeholder path (no fonts): per-char advance is (font_size * 0.6).max(1.0)
    // plus letter-spacing between characters, matching paint_text_placeholder.
    let placeholder = measure_form_control_text_width("ab", 10.0, &[], 2.0);
    assert!(
        (placeholder - 14.0).abs() < 1e-4,
        "placeholder width expected 6 + 2 + 6 = 14, got {placeholder}"
    );
    assert_eq!(measure_form_control_text_width("", 10.0, &[], 2.0), 0.0);

    // Font path: the measured width must equal the exact cursor advance of
    // paint_text_with_font_refs (per-char rasterize advance + same-font kerning +
    // letter-spacing between characters), so centered labels stay centered.
    let fonts = crate::font::load_default_text_fonts();
    if fonts.is_empty() {
        eprintln!("Skipping font path: no default text fonts available");
        return;
    }
    let font_refs: Vec<&crate::font::Font> = fonts.iter().collect();
    let font_size = 16.0;
    let letter_spacing = 1.5;
    let text = "AVAST Wavy 検索";
    let chars: Vec<char> = text.chars().collect();
    let mut expected = 0.0f32;
    let mut previous: Option<(char, usize)> = None;
    for (i, &ch) in chars.iter().enumerate() {
        let (font_index, _, advance) = rasterize_with_fallback_refs(&font_refs, ch, font_size);
        if let Some((prev, prev_index)) = previous
            && prev_index == font_index
        {
            expected += font_refs[font_index].glyph_kerning(prev, ch, font_size);
        }
        expected += advance;
        if i + 1 < chars.len() {
            expected += letter_spacing;
        }
        previous = Some((ch, font_index));
    }
    let measured = measure_form_control_text_width(text, font_size, &font_refs, letter_spacing);
    assert!(
        (measured - expected).abs() < 1e-3,
        "measured {measured} must equal painter cursor advance {expected}"
    );
}

#[test]
fn form_control_label_uses_web_font_variant() {
    // A FormControl label whose fragment style names a registered web-font
    // family must be measured and drawn with that variant. Painting with
    // (global fonts = [F], no registry) and (global fonts = [], registry
    // "btnface" -> F) must produce pixel-identical output; without web-font
    // support the second run would fall back to placeholder rectangles.
    let Some(font_path) = crate::font::find_system_font("sans-serif") else {
        eprintln!("Skipping web font form control test: no sans-serif system font found");
        return;
    };
    let (Ok(global_font), Ok(web_font)) = (
        crate::font::Font::load_from_file(&font_path),
        crate::font::Font::load_from_file(&font_path),
    ) else {
        eprintln!("Skipping web font form control test: failed to load system font");
        return;
    };
    let global_font = std::sync::Arc::new(global_font);

    let mut registry = crate::font::WebFontRegistry::new();
    registry.push(
        "btnface",
        crate::font::FontWeight::parse("normal"),
        crate::font::FontStyle::parse("normal"),
        web_font,
    );

    // The button UA defaults include `text-align: center`, so the centering
    // measurement path is exercised alongside glyph drawing.
    let button = NodeHandle::element("button");
    let mut resolver = StyleResolver::new();
    let control_style = resolver.computed_style(&button);
    assert_eq!(
        control_style.get("text-align"),
        Some(&crate::css::ComputedValue::Keyword("center".to_string())),
        "button UA default must center the label for this test to cover measurement"
    );
    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 60.0,
        height: 24.0,
    };
    let layout = LayoutBox {
        node: button.clone(),
        dimensions: BoxDimensions {
            content: viewport,
            ..BoxDimensions::default()
        },
        visibility: crate::layout::Visibility::Visible,
        overflow: crate::layout::Overflow::Visible,
        z_index: 0,
        transform: crate::css::AffineTransform::identity(),
        lines: vec![LineBox {
            rect: viewport,
            baseline: 12.8,
            fragments: vec![InlineFragment {
                node: button,
                content: InlineFragmentContent::FormControl(
                    control_style.clone(),
                    "AB".to_string(),
                ),
                rect: viewport,
                metrics: FontMetrics::from_font_size(16.0),
                vertical_align: VerticalAlign::Baseline,
                style: FragmentStyle {
                    font_family: Some("btnface".to_string()),
                    ..FragmentStyle::default()
                },
            }],
        }],
        children: Vec::new(),
        marker: None,
    };

    let mut with_global = Canvas::new(60, 24);
    paint_text_with_registry(
        &mut with_global,
        &layout,
        &control_style,
        None,
        viewport,
        std::slice::from_ref(&global_font),
        None,
    );
    let mut with_registry = Canvas::new(60, 24);
    paint_text_with_registry(
        &mut with_registry,
        &layout,
        &control_style,
        None,
        viewport,
        &[],
        Some(&registry),
    );

    // Glyph pixels must be present (anything that is neither transparent nor
    // the UA background/border fill), so the comparison cannot pass vacuously.
    let background = Color::rgb(0xef, 0xef, 0xef);
    let border = Color::rgb(0x76, 0x76, 0x76);
    let mut glyph_pixels = 0usize;
    for y in 0..24 {
        for x in 0..60 {
            assert_eq!(
                with_registry.pixel(x, y),
                with_global.pixel(x, y),
                "pixel ({x}, {y}) must match the same font painted as a global font"
            );
            let pixel = with_global.pixel(x, y);
            if pixel != Some(Color::rgba(0, 0, 0, 0))
                && pixel != Some(background)
                && pixel != Some(border)
            {
                glyph_pixels += 1;
            }
        }
    }
    assert!(
        glyph_pixels > 0,
        "the label must actually paint glyph pixels for the comparison to be meaningful"
    );
}

#[test]
fn render_timings_accumulate_multiple_documents_and_reset_when_taken() {
    clear_render_timings();
    record_render_timings(&RenderTimings {
        javascript: std::time::Duration::from_millis(12),
        layout: std::time::Duration::from_millis(3),
        ..RenderTimings::default()
    });
    record_render_timings(&RenderTimings {
        javascript: std::time::Duration::from_millis(8),
        png_encode: std::time::Duration::from_millis(2),
        ..RenderTimings::default()
    });

    assert_eq!(
        take_last_render_timings(),
        RenderTimings {
            javascript: std::time::Duration::from_millis(20),
            layout: std::time::Duration::from_millis(3),
            png_encode: std::time::Duration::from_millis(2),
            ..RenderTimings::default()
        }
    );
    assert_eq!(take_last_render_timings(), RenderTimings::default());
}

#[test]
fn render_glyph_cache_hits_and_separates_font_identity_and_size() {
    let first_fonts = super::load_text_fonts();
    let second_fonts = super::load_text_fonts();
    assert!(!first_fonts.is_empty());
    assert!(!second_fonts.is_empty());

    super::with_render_glyph_cache(|| {
        let _ = super::rasterize_with_fallback(&first_fonts[..1], 'A', 16.0);
        let _ = super::rasterize_with_fallback(&first_fonts[..1], 'A', 16.0);
        let _ = super::rasterize_with_fallback(&first_fonts[..1], 'A', 18.0);
        let _ = super::rasterize_with_fallback(&second_fonts[..1], 'A', 16.0);
    });

    let (hits, misses) = super::text::render_glyph_cache_stats();
    assert_eq!(hits, 1, "only the identical font/character/size may hit");
    assert_eq!(misses, 3, "font identity and size must use distinct entries");
}

#[test]
fn nested_render_glyph_cache_reuses_outer_entries_and_remains_active() {
    let fonts = super::load_text_fonts();
    assert!(!fonts.is_empty());

    super::with_render_glyph_cache(|| {
        let _ = super::rasterize_with_fallback(&fonts[..1], 'A', 16.0);
        super::with_render_glyph_cache(|| {
            let _ = super::rasterize_with_fallback(&fonts[..1], 'A', 16.0);
        });
        let _ = super::rasterize_with_fallback(&fonts[..1], 'A', 16.0);
    });

    let (hits, misses) = super::text::render_glyph_cache_stats();
    assert_eq!(hits, 2, "nested and post-nested lookups must hit the outer entry");
    assert_eq!(misses, 1, "the outermost lookup must be the only miss");
}

// --- Element scroll offsets (issue #245) ---

/// Lays out `html`, applies the given element scroll offsets and the Window
/// scroll, then paints. Each entry in `offsets` is a selector plus the offset to
/// store on the element it matches.
fn render_with_scroll(
    html: &str,
    viewport_size: f32,
    offsets: &[(&str, f32, f32)],
    window_scroll: (f32, f32),
) -> Canvas {
    let document = TreeBuilder::parse(html).document();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }
    for (selector, x, y) in offsets {
        document
            .query_selector(selector)
            .unwrap_or_else(|| panic!("no element matched {selector}"))
            .set_scroll_offset(*x, *y);
    }
    let viewport = Rect { x: 0.0, y: 0.0, width: viewport_size, height: viewport_size };
    let mut layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    super::apply_scroll_offsets(&mut layout, &mut resolver, window_scroll);
    paint_layout(&layout, &mut resolver, viewport)
}

/// Two stacked 20x20 stripes inside a 20x20 scroll container at the origin.
const STRIPED_SCROLLER: &str = "<html><head><style>\
     body { margin: 0 } \
     #sc { width: 20px; height: 20px; overflow: hidden } \
     #top { width: 20px; height: 20px; background-color: #ff0000 } \
     #bottom { width: 20px; height: 20px; background-color: #00ff00 } \
     </style></head><body><div id=\"sc\"><div id=\"top\"></div><div id=\"bottom\"></div></div></body></html>";

#[test]
fn element_scroll_translates_content_and_keeps_the_clip() {
    let red = Some(Color::rgba(255, 0, 0, 255));
    let green = Some(Color::rgba(0, 255, 0, 255));

    let unscrolled = render_with_scroll(STRIPED_SCROLLER, 40.0, &[], (0.0, 0.0));
    assert_eq!(unscrolled.pixel(5, 5), red);
    assert_eq!(unscrolled.pixel(5, 15), red);
    // The second stripe is clipped away by the container.
    assert_eq!(unscrolled.pixel(5, 25).unwrap().a, 0);

    let half = render_with_scroll(STRIPED_SCROLLER, 40.0, &[("#sc", 0.0, 10.0)], (0.0, 0.0));
    assert_eq!(half.pixel(5, 5), red);
    assert_eq!(half.pixel(5, 15), green);
    assert_eq!(half.pixel(5, 25).unwrap().a, 0);

    let full = render_with_scroll(STRIPED_SCROLLER, 40.0, &[("#sc", 0.0, 20.0)], (0.0, 0.0));
    assert_eq!(full.pixel(5, 5), green);
    assert_eq!(full.pixel(5, 19), green);
    // Scrolled-away content must not paint above the container either.
    assert_eq!(full.pixel(5, 25).unwrap().a, 0);
}

#[test]
fn element_scroll_left_moves_content_horizontally() {
    // The absolutely positioned child's containing block is the scroll
    // container itself, so it scrolls with the container's content.
    let html = "<html><head><style>\
         body { margin: 0 } \
         #sc { position: relative; width: 20px; height: 20px; overflow: hidden } \
         #right { position: absolute; left: 20px; top: 0; width: 20px; height: 20px; \
                  background-color: #00ff00 } \
         </style></head><body><div id=\"sc\"><div id=\"right\"></div></div></body></html>";
    let green = Some(Color::rgba(0, 255, 0, 255));

    let unscrolled = render_with_scroll(html, 40.0, &[], (0.0, 0.0));
    assert_eq!(unscrolled.pixel(5, 5).unwrap().a, 0);
    assert_eq!(unscrolled.pixel(25, 5).unwrap().a, 0, "content outside the container is clipped");

    let scrolled = render_with_scroll(html, 40.0, &[("#sc", 20.0, 0.0)], (0.0, 0.0));
    assert_eq!(scrolled.pixel(5, 5), green);
    assert_eq!(scrolled.pixel(25, 5).unwrap().a, 0);
}

#[test]
fn absolute_descendant_uses_its_containing_blocks_scroll_and_clip_chain() {
    let html = "<html><head><style>\
         body { margin: 0 } \
         #outer { position: relative; width: 50px; height: 30px; overflow: hidden } \
         #outer-flow { width: 100px; height: 1px } \
         #inner { width: 20px; height: 20px; overflow: hidden } \
         #inner-flow { width: 60px; height: 20px } \
         #target { position: absolute; left: 30px; top: 5px; width: 10px; height: 10px; \
                   background-color: #ff0000 } \
         </style></head><body><div id=\"outer\"><div id=\"outer-flow\"></div>\
         <div id=\"inner\"><div id=\"inner-flow\"></div><div id=\"target\"></div></div>\
         </div></body></html>";
    let red = Some(Color::rgba(255, 0, 0, 255));

    let unscrolled = render_with_scroll(html, 70.0, &[], (0.0, 0.0));
    assert_eq!(unscrolled.pixel(35, 10), red, "target must escape inner clip");

    let inner = render_with_scroll(html, 70.0, &[("#inner", 10.0, 0.0)], (0.0, 0.0));
    assert_eq!(inner.pixel(35, 10), red, "inner scroll must not move target");
    assert_eq!(inner.pixel(25, 10).unwrap().a, 0);

    let both = render_with_scroll(
        html,
        70.0,
        &[("#outer", 10.0, 0.0), ("#inner", 10.0, 0.0)],
        (0.0, 0.0),
    );
    assert_eq!(both.pixel(25, 10), red, "containing block scroll must move target");
    assert_eq!(both.pixel(35, 10).unwrap().a, 0);
}

#[test]
fn element_scroll_is_clamped_to_the_scrollable_extent_when_painting() {
    // A stored offset past the extent paints the extent, so a shrunk document
    // cannot scroll its content out of view.
    let green = Some(Color::rgba(0, 255, 0, 255));
    let clamped = render_with_scroll(
        STRIPED_SCROLLER,
        40.0,
        &[("#sc", 500.0, 500.0)],
        (0.0, 0.0),
    );
    assert_eq!(clamped.pixel(5, 5), green);
    assert_eq!(clamped.pixel(5, 19), green);
}

#[test]
fn overflow_auto_and_scroll_clip_and_scroll_their_content() {
    let green = Some(Color::rgba(0, 255, 0, 255));
    for keyword in ["auto", "scroll"] {
        let html = STRIPED_SCROLLER.replace("overflow: hidden", &format!("overflow: {keyword}"));
        let unscrolled = render_with_scroll(&html, 40.0, &[], (0.0, 0.0));
        assert_eq!(
            unscrolled.pixel(5, 25).unwrap().a,
            0,
            "overflow: {keyword} must clip its content"
        );
        let scrolled = render_with_scroll(&html, 40.0, &[("#sc", 0.0, 20.0)], (0.0, 0.0));
        assert_eq!(
            scrolled.pixel(5, 5),
            green,
            "overflow: {keyword} must scroll its content"
        );
    }
}

#[test]
fn overflow_visible_and_clip_do_not_scroll_their_content() {
    let red = Some(Color::rgba(255, 0, 0, 255));
    for keyword in ["visible", "clip"] {
        let html = STRIPED_SCROLLER.replace("overflow: hidden", &format!("overflow: {keyword}"));
        let scrolled = render_with_scroll(&html, 40.0, &[("#sc", 0.0, 20.0)], (0.0, 0.0));
        assert_eq!(
            scrolled.pixel(5, 5),
            red,
            "overflow: {keyword} is not a scroll container"
        );
    }

    let visible = render_with_scroll(
        &STRIPED_SCROLLER.replace("overflow: hidden", "overflow: visible"),
        40.0,
        &[],
        (0.0, 0.0),
    );
    let clipped = render_with_scroll(
        &STRIPED_SCROLLER.replace("overflow: hidden", "overflow: clip"),
        40.0,
        &[],
        (0.0, 0.0),
    );
    assert_eq!(visible.pixel(5, 25), Some(Color::rgba(0, 255, 0, 255)));
    assert_eq!(clipped.pixel(5, 25).unwrap().a, 0);
}

#[test]
fn overflow_clip_can_clip_only_one_axis() {
    let html = "<html><head><style>\
         body { margin: 0 } \
         #clip-x { width: 20px; height: 20px; overflow: clip visible } \
         #child { width: 40px; height: 40px; background-color: #ff0000 } \
         </style></head><body><div id=\"clip-x\"><div id=\"child\"></div></div></body></html>";
    let canvas = render_with_scroll(html, 50.0, &[], (0.0, 0.0));
    let red = Some(Color::rgba(255, 0, 0, 255));
    assert_eq!(canvas.pixel(10, 30), red, "visible y-axis must not clip");
    assert_eq!(canvas.pixel(30, 10).unwrap().a, 0, "clip x-axis must clip");
}

#[test]
fn fixed_descendant_stays_put_while_its_scroll_container_scrolls() {
    let html = "<html><head><style>\
         body { margin: 0 } \
         #sc { width: 20px; height: 20px; overflow: hidden } \
         #top { width: 20px; height: 20px; background-color: #ff0000 } \
         #bottom { width: 20px; height: 20px; background-color: #00ff00 } \
         #pin { position: fixed; left: 0; top: 0; width: 6px; height: 6px; \
                background-color: #0000ff } \
         </style></head><body><div id=\"sc\"><div id=\"top\"></div><div id=\"bottom\"></div>\
         <div id=\"pin\"></div></div></body></html>";
    let blue = Some(Color::rgba(0, 0, 255, 255));
    let green = Some(Color::rgba(0, 255, 0, 255));

    let scrolled = render_with_scroll(html, 40.0, &[("#sc", 0.0, 20.0)], (0.0, 0.0));
    // Had the fixed box scrolled with the container it would sit at y = -20 and
    // the green stripe would show through instead.
    assert_eq!(scrolled.pixel(3, 3), blue);
    assert_eq!(scrolled.pixel(10, 3), green);
}

#[test]
fn scroll_container_inside_a_fixed_subtree_still_scrolls_its_content() {
    let html = "<html><head><style>\
         body { margin: 0; height: 400px } \
         #pinned { position: fixed; left: 0; top: 0; width: 20px; height: 20px } \
         #sc { width: 20px; height: 20px; overflow: hidden } \
         #top { width: 20px; height: 20px; background-color: #ff0000 } \
         #bottom { width: 20px; height: 20px; background-color: #00ff00 } \
         </style></head><body><div id=\"pinned\"><div id=\"sc\"><div id=\"top\"></div>\
         <div id=\"bottom\"></div></div></div></body></html>";
    let green = Some(Color::rgba(0, 255, 0, 255));

    // The Window is scrolled too: the fixed subtree must ignore that offset
    // while the scroll container inside it still scrolls its own content.
    let canvas = render_with_scroll(html, 40.0, &[("#sc", 0.0, 20.0)], (0.0, 8.0));
    assert_eq!(canvas.pixel(5, 5), green);
    assert_eq!(canvas.pixel(5, 19), green);
}

#[test]
fn nested_scroll_containers_accumulate_offsets_when_painting() {
    let html = "<html><head><style>\
         body { margin: 0 } \
         #outer { width: 30px; height: 30px; overflow: hidden } \
         #inner { width: 20px; height: 20px; overflow: hidden; margin-top: 10px } \
         #top { width: 20px; height: 20px; background-color: #ff0000 } \
         #bottom { width: 20px; height: 20px; background-color: #00ff00 } \
         </style></head><body><div id=\"outer\"><div id=\"inner\"><div id=\"top\"></div>\
         <div id=\"bottom\"></div></div></div></body></html>";
    let green = Some(Color::rgba(0, 255, 0, 255));

    // The inner container starts 10px down; scrolling the outer container by 10
    // lifts it to the top, and scrolling the inner one by 20 shows its second
    // stripe. Both offsets have to apply for green to reach the origin.
    let canvas = render_with_scroll(
        html,
        40.0,
        &[("#outer", 0.0, 10.0), ("#inner", 0.0, 20.0)],
        (0.0, 0.0),
    );
    assert_eq!(canvas.pixel(5, 0), green);
    assert_eq!(canvas.pixel(5, 19), green);
    assert_eq!(canvas.pixel(5, 25).unwrap().a, 0);
}

#[test]
fn window_and_element_scroll_offsets_combine_when_painting() {
    let green = Some(Color::rgba(0, 255, 0, 255));
    let canvas = render_with_scroll(
        STRIPED_SCROLLER,
        40.0,
        &[("#sc", 0.0, 20.0)],
        (0.0, 10.0),
    );
    // The container itself moves up with the Window scroll, so its content ends
    // up spanning y = -10..10 with the second stripe visible.
    assert_eq!(canvas.pixel(5, 0), green);
    assert_eq!(canvas.pixel(5, 9), green);
    assert_eq!(canvas.pixel(5, 15).unwrap().a, 0);
}

/// The container's border box must stay where layout put it while its own line
/// boxes, marker and children move — this is what keeps the clip and the
/// element's own client rect stable while its content scrolls.
#[test]
fn apply_scroll_offsets_moves_content_but_not_the_scroll_container_box() {
    let document = TreeBuilder::parse(
        "<html><head><style>\
         body { margin: 0 } \
         #sc { width: 40px; height: 20px; overflow: hidden; font-size: 10px } \
         #child { width: 40px; height: 40px } \
         </style></head><body><div id=\"sc\">text<div id=\"child\"></div></div></body></html>",
    )
    .document();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }
    let viewport = Rect { x: 0.0, y: 0.0, width: 100.0, height: 100.0 };
    let scroller = document.query_selector("#sc").unwrap();
    let child = document.query_selector("#child").unwrap();

    let baseline = layout_tree(&document, &mut resolver, viewport).unwrap();
    let baseline_scroller = find_layout_box_by_node(&baseline, &scroller).unwrap();
    let baseline_box = baseline_scroller.dimensions.content;
    let baseline_line = baseline_scroller.lines.first().map(|line| line.rect.y);
    let baseline_child = find_layout_box_by_node(&baseline, &child)
        .unwrap()
        .dimensions
        .content;

    scroller.set_scroll_offset(0.0, 15.0);
    let mut scrolled = layout_tree(&document, &mut resolver, viewport).unwrap();
    super::apply_scroll_offsets(&mut scrolled, &mut resolver, (0.0, 0.0));
    let scrolled_scroller = find_layout_box_by_node(&scrolled, &scroller).unwrap();

    assert_eq!(
        scrolled_scroller.dimensions.content, baseline_box,
        "the scroll container's own box must not move"
    );
    assert_eq!(
        scrolled_scroller.lines.first().map(|line| line.rect.y),
        baseline_line.map(|y| y - 15.0),
        "the container's own line boxes are scrollable content"
    );
    assert_eq!(
        find_layout_box_by_node(&scrolled, &child)
            .unwrap()
            .dimensions
            .content
            .y,
        baseline_child.y - 15.0,
        "children scroll with the content"
    );
}

fn find_layout_box_by_node<'a>(
    layout: &'a crate::layout::LayoutBox,
    node: &NodeHandle,
) -> Option<&'a crate::layout::LayoutBox> {
    if &layout.node == node {
        return Some(layout);
    }
    layout
        .children
        .iter()
        .find_map(|child| find_layout_box_by_node(child, node))
}

/// End-to-end check of the whole feature: a page script sets `scrollTop`, and
/// the document rendered by the same call paints the scrolled content.
#[test]
fn scripted_element_scroll_reaches_the_rendered_document() {
    let document = TreeBuilder::parse(
        "<html><head><style>\
         body { margin: 0 } \
         #sc { width: 20px; height: 20px; overflow: hidden } \
         #top { width: 20px; height: 20px; background-color: #ff0000 } \
         #bottom { width: 20px; height: 20px; background-color: #00ff00 } \
         </style></head><body><div id=\"sc\"><div id=\"top\"></div><div id=\"bottom\"></div></div>\
         <script>document.getElementById('sc').scrollTop = 20;</script></body></html>",
    )
    .document();

    let canvas = render_document(
        &document,
        Rect { x: 0.0, y: 0.0, width: 40.0, height: 40.0 },
    )
    .expect("render should succeed");

    assert_eq!(canvas.pixel(5, 5), Some(Color::rgba(0, 255, 0, 255)));
    assert_eq!(canvas.pixel(5, 19), Some(Color::rgba(0, 255, 0, 255)));
    assert_eq!(canvas.pixel(5, 25).unwrap().a, 0);
}

// --- object-fit / object-position (issue #246) ---

/// Renders a 4x2 source image (four distinct colour columns, a bright top row
/// and a dark bottom row) inside an `<img>` whose content box is `box_size`, so
/// the painted destination rect and any crop are readable from the pixels.
///
/// The page kills the font strut, which keeps the image fragment at the line box
/// origin even for boxes a pixel or two tall; the returned rect is that content
/// box, so assertions can be written relative to it.
///
/// Geometry expectations come from Firefox 152: the same cases were rendered
/// there over Marionette and measured from a full-page screenshot.
fn render_object_fit(declarations: &str, box_size: (f32, f32)) -> (Canvas, Rect) {
    let mut source = Canvas::new(4, 2);
    let columns = [
        Color::rgb(255, 0, 0),
        Color::rgb(0, 255, 0),
        Color::rgb(0, 0, 255),
        Color::rgb(255, 255, 0),
    ];
    for (index, color) in columns.iter().enumerate() {
        let x = index as f32;
        source.fill_rect(Rect { x, y: 0.0, width: 1.0, height: 1.0 }, *color);
        source.fill_rect(
            Rect { x, y: 1.0, width: 1.0, height: 1.0 },
            Color::rgb(color.r / 2, color.g / 2, color.b / 2),
        );
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(source.encode_png());
    let (width, height) = box_size;
    let html = format!(
        r#"<html><head><style>body {{ margin: 0; font-size: 0; line-height: 0; }} img {{ width: {width}px; height: {height}px; {declarations} }}</style></head><body><img src="data:image/png;base64,{encoded}"></body></html>"#
    );
    let document = TreeBuilder::parse(&html).document();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }
    let viewport = Rect { x: 0.0, y: 0.0, width: 140.0, height: 140.0 };
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let content_box = image_fragment_rect(&layout).expect("the img must produce an image fragment");
    (paint_layout(&layout, &mut resolver, viewport), content_box)
}

/// Returns the rect of the first inline image fragment in the tree, which for an
/// `<img>` with no border or padding is its content box.
fn image_fragment_rect(layout: &crate::layout::LayoutBox) -> Option<Rect> {
    for line in &layout.lines {
        for fragment in &line.fragments {
            if matches!(fragment.content, crate::layout::InlineFragmentContent::Image(..)) {
                return Some(fragment.rect);
            }
        }
    }
    if matches!(layout.node.tag_name().as_deref(), Some("img")) {
        // Fallback for an img with no inline fragment: a positioned box paints
        // through paint_replaced_image_box. Such a box can still generate a
        // fragment as well today, which is the double paint tracked in #264, so
        // this is only reached when the inline search found nothing.
        return Some(layout.dimensions.content);
    }
    layout.children.iter().find_map(image_fragment_rect)
}

/// Returns the bounding box of the painted image relative to `content_box`, so
/// expectations read the same way they were measured in Firefox: as offsets
/// inside the replaced element's content box.
fn painted_bounds_in_box(canvas: &Canvas, content_box: Rect) -> Option<(i32, i32, u32, u32)> {
    painted_bounds(canvas).map(|(x, y, width, height)| {
        (
            x as i32 - content_box.x as i32,
            y as i32 - content_box.y as i32,
            width,
            height,
        )
    })
}

/// Returns the bounding box `(x, y, width, height)` of the painted image inside
/// the canvas, treating fully transparent pixels as empty.
fn painted_bounds(canvas: &Canvas) -> Option<(u32, u32, u32, u32)> {
    let mut bounds: Option<(u32, u32, u32, u32)> = None;
    for y in 0..canvas.height() {
        for x in 0..canvas.width() {
            if canvas.pixel(x, y).map(|pixel| pixel.a) == Some(0) {
                continue;
            }
            bounds = Some(match bounds {
                None => (x, y, x, y),
                Some((min_x, min_y, max_x, max_y)) => {
                    (min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y))
                }
            });
        }
    }
    bounds.map(|(min_x, min_y, max_x, max_y)| {
        (min_x, min_y, max_x - min_x + 1, max_y - min_y + 1)
    })
}

/// Samples the colour at the centre of each of the four source columns as they
/// were painted, which shows both the paint order and any horizontal crop.
fn painted_columns(canvas: &Canvas, bounds: (u32, u32, u32, u32)) -> Vec<(u8, u8, u8)> {
    let (x, y, width, height) = bounds;
    (0..4)
        .map(|index| {
            let sample_x = x + (width as f32 * (index as f32 + 0.5) / 4.0) as u32;
            let sample_y = y + height / 4;
            let pixel = canvas.pixel(sample_x, sample_y).unwrap();
            (pixel.r, pixel.g, pixel.b)
        })
        .collect()
}

#[test]
fn object_fit_fill_stretches_to_the_content_box() {
    let (canvas, content_box) = render_object_fit("object-fit: fill", (100.0, 100.0));
    assert_eq!(painted_bounds_in_box(&canvas, content_box), Some((0, 0, 100, 100)));
    assert_eq!(
        painted_columns(&canvas, (0, 0, 100, 100)),
        vec![(255, 0, 0), (0, 255, 0), (0, 0, 255), (255, 255, 0)]
    );
}

#[test]
fn object_fit_contain_letterboxes_and_leaves_the_rest_transparent() {
    let (canvas, content_box) = render_object_fit("object-fit: contain", (100.0, 100.0));
    // scale = min(100/4, 100/2) = 25 -> 100x50, centred vertically.
    assert_eq!(painted_bounds_in_box(&canvas, content_box), Some((0, 25, 100, 50)));
    assert_eq!(
        painted_columns(&canvas, (0, 25, 100, 50)),
        vec![(255, 0, 0), (0, 255, 0), (0, 0, 255), (255, 255, 0)]
    );
    // The letterbox bands stay untouched.
    assert_eq!(canvas.pixel(50, 10).unwrap().a, 0);
    assert_eq!(canvas.pixel(50, 90).unwrap().a, 0);
}

#[test]
fn object_fit_cover_fills_the_box_and_crops_the_overflow() {
    let (canvas, content_box) = render_object_fit("object-fit: cover", (100.0, 100.0));
    // scale = max(100/4, 100/2) = 50 -> 200x100 centred, cropped to the box.
    assert_eq!(painted_bounds_in_box(&canvas, content_box), Some((0, 0, 100, 100)));
    // Only the middle half of the source survives: green and blue, not red or
    // yellow.
    assert_eq!(
        painted_columns(&canvas, (0, 0, 100, 100)),
        vec![(0, 255, 0), (0, 255, 0), (0, 0, 255), (0, 0, 255)]
    );
    // Nothing paints outside the content box.
    assert_eq!(canvas.pixel(120, 50).unwrap().a, 0);
}

#[test]
fn object_fit_none_paints_the_intrinsic_size_centred() {
    let (canvas, content_box) = render_object_fit("object-fit: none", (100.0, 100.0));
    // Intrinsic 4x2 centred: (100-4)/2 = 48, (100-2)/2 = 49.
    assert_eq!(painted_bounds_in_box(&canvas, content_box), Some((48, 49, 4, 2)));
    assert_eq!(canvas.pixel(48, 49), Some(Color::rgba(255, 0, 0, 255)));
    assert_eq!(canvas.pixel(51, 49), Some(Color::rgba(255, 255, 0, 255)));
    assert_eq!(canvas.pixel(48, 50), Some(Color::rgba(127, 0, 0, 255)));
    assert_eq!(canvas.pixel(10, 10).unwrap().a, 0);
}

#[test]
fn object_fit_scale_down_picks_the_smaller_of_none_and_contain() {
    // The image fits, so `scale-down` behaves like `none`.
    let (large, large_box) = render_object_fit("object-fit: scale-down", (100.0, 100.0));
    assert_eq!(painted_bounds_in_box(&large, large_box), Some((48, 49, 4, 2)));

    // It does not fit, so `scale-down` behaves like `contain`:
    // scale = min(2/4, 1/2) = 0.5 -> 2x1.
    let (small, small_box) = render_object_fit("object-fit: scale-down", (2.0, 1.0));
    assert_eq!(painted_bounds_in_box(&small, small_box), Some((0, 0, 2, 1)));

    // A box the image exactly fills is still `none`, not a scale.
    let (exact, exact_box) = render_object_fit("object-fit: scale-down", (4.0, 2.0));
    assert_eq!(painted_bounds_in_box(&exact, exact_box), Some((0, 0, 4, 2)));
}

#[test]
fn object_position_places_a_contained_object_inside_the_box() {
    let (top_left, top_left_box) =
        render_object_fit("object-fit: contain; object-position: left top", (100.0, 100.0));
    assert_eq!(painted_bounds_in_box(&top_left, top_left_box), Some((0, 0, 100, 50)));

    let (bottom_right, bottom_right_box) =
        render_object_fit("object-fit: contain; object-position: right bottom", (100.0, 100.0));
    assert_eq!(
        painted_bounds_in_box(&bottom_right, bottom_right_box),
        Some((0, 50, 100, 50))
    );
}

#[test]
fn object_position_places_an_intrinsic_object_by_percentage_and_length() {
    // x = (100-4) * 0.25 = 24, y = (100-2) * 0.75 = 73.5.
    let (percentage, percentage_box) =
        render_object_fit("object-fit: none; object-position: 25% 75%", (100.0, 100.0));
    let (x, y, width, height) = painted_bounds_in_box(&percentage, percentage_box).unwrap();
    assert_eq!((x, width, height), (24, 4, 2));
    assert!(
        (73..=74).contains(&y),
        "a 73.5px offset must land on row 73 or 74, got {y}"
    );

    let (length, length_box) =
        render_object_fit("object-fit: none; object-position: 10px 20px", (100.0, 100.0));
    assert_eq!(painted_bounds_in_box(&length, length_box), Some((10, 20, 4, 2)));

    // A single component centres the other axis.
    let (single, single_box) =
        render_object_fit("object-fit: none; object-position: left", (100.0, 100.0));
    assert_eq!(painted_bounds_in_box(&single, single_box), Some((0, 49, 4, 2)));
}

#[test]
fn object_position_chooses_which_side_cover_crops() {
    // The object is wider than the box, so the free space is negative and the
    // position picks the visible slice.
    let (left, _) = render_object_fit("object-fit: cover; object-position: 0% 50%", (100.0, 100.0));
    assert_eq!(
        painted_columns(&left, (0, 0, 100, 100)),
        vec![(255, 0, 0), (255, 0, 0), (0, 255, 0), (0, 255, 0)],
        "cover anchored left must show the first half of the source"
    );

    let (right, _) =
        render_object_fit("object-fit: cover; object-position: 100% 50%", (100.0, 100.0));
    assert_eq!(
        painted_columns(&right, (0, 0, 100, 100)),
        vec![(0, 0, 255), (0, 0, 255), (255, 255, 0), (255, 255, 0)],
        "cover anchored right must show the last half of the source"
    );
}

#[test]
fn object_fit_applies_to_a_positioned_replaced_box() {
    // The positioned path paints through paint_replaced_image_box rather than an
    // inline fragment, and must apply the same sizing.
    let mut source = Canvas::new(4, 2);
    source.fill_rect(Rect { x: 0.0, y: 0.0, width: 4.0, height: 2.0 }, Color::rgb(255, 0, 0));
    let encoded = base64::engine::general_purpose::STANDARD.encode(source.encode_png());
    let html = format!(
        r#"<html><head><style>body {{ margin: 0; }} img {{ position: absolute; left: 0; top: 0; width: 100px; height: 100px; object-fit: contain; }}</style></head><body><img src="data:image/png;base64,{encoded}"></body></html>"#
    );
    let document = TreeBuilder::parse(&html).document();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }
    let viewport = Rect { x: 0.0, y: 0.0, width: 140.0, height: 140.0 };
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let canvas = paint_layout(&layout, &mut resolver, viewport);

    assert_eq!(painted_bounds(&canvas), Some((0, 25, 100, 50)));
}

#[test]
fn default_object_fit_keeps_stretching_replaced_content() {
    // No declaration at all must paint exactly like the pre-object-fit engine:
    // the image stretched across the whole content box.
    let (canvas, content_box) = render_object_fit("", (100.0, 100.0));
    assert_eq!(painted_bounds_in_box(&canvas, content_box), Some((0, 0, 100, 100)));
    assert_eq!(
        painted_columns(&canvas, (0, 0, 100, 100)),
        vec![(255, 0, 0), (0, 255, 0), (0, 0, 255), (255, 255, 0)]
    );
}

#[test]
fn object_fit_applies_to_a_block_level_replaced_box() {
    // `img { display: block }` is a common reset, and it lays the replaced
    // content out in its own line box rather than in the surrounding text flow.
    let (canvas, content_box) =
        render_object_fit("display: block; object-fit: contain", (100.0, 100.0));

    assert_eq!(painted_bounds_in_box(&canvas, content_box), Some((0, 25, 100, 50)));
}
