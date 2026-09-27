use omoikane::html::TreeBuilder;
use omoikane::layout::Rect;
use omoikane::paint::{Canvas, Color, render_document_pages, render_document_pages_with_url};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::{Duration, Instant};

fn colored_bounds(
    page: &Canvas,
    area: (u32, u32, u32, u32),
    predicate: impl Fn(Color) -> bool,
) -> Option<(u32, u32, u32, u32)> {
    let (left, top, right, bottom) = area;
    let mut bounds: Option<(u32, u32, u32, u32)> = None;
    for y in top..bottom {
        for x in left..right {
            if page.pixel(x, y).is_some_and(&predicate) {
                bounds = Some(match bounds {
                    Some((min_x, min_y, max_x, max_y)) => {
                        (min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y))
                    }
                    None => (x, y, x, y),
                });
            }
        }
    }
    bounds
}

#[test]
fn printed_margin_text_uses_page_color_font_and_box_alignment() {
    let document =
        TreeBuilder::parse(include_str!("fixtures/print/page-margin-text.html")).document();
    let pages = render_document_pages(
        &document,
        Rect {
            width: 200.0,
            height: 200.0,
            ..Rect::default()
        },
    )
    .unwrap();
    assert_eq!(pages.len(), 1);
    let page = &pages[0];
    let top_red = colored_bounds(page, (40, 0, 160, 40), |pixel| {
        pixel.r > 200 && pixel.g < 100 && pixel.b < 100
    })
    .expect("top-left red text");
    let top_blue = colored_bounds(page, (40, 0, 160, 40), |pixel| {
        pixel.b > 200 && pixel.r < 100 && pixel.g < 100
    })
    .expect("top-center blue text");
    let top_purple = colored_bounds(page, (40, 0, 160, 40), |pixel| {
        pixel.r > 100 && pixel.b > 100 && pixel.g < 100
    })
    .expect("top-right purple text");
    assert!(top_red.0 < top_blue.0 && top_blue.0 < top_purple.0);
    let left_green = colored_bounds(page, (0, 40, 40, 160), |pixel| {
        pixel.g > 100 && pixel.r < 100 && pixel.b < 100
    })
    .expect("left-middle green text");
    let right_orange = colored_bounds(page, (160, 40, 200, 160), |pixel| {
        pixel.r > 100 && pixel.g > 50 && pixel.b < 100
    })
    .expect("right-middle orange text");
    assert!(left_green.1 > right_orange.1);
    assert!(top_red.3 - top_red.1 > left_green.3 - left_green.1);
    assert!(
        colored_bounds(page, (40, 160, 100, 200), |pixel| {
            pixel.r > 100 && pixel.g < 80 && pixel.b < 80
        })
        .is_some()
    );
    assert!(
        colored_bounds(page, (40, 160, 160, 200), |pixel| {
            pixel.r < 100 && pixel.g < 100 && pixel.b < 100
        })
        .is_some()
    );
    assert_eq!(page.pixel(80, 80), Some(Color::rgb(255, 255, 255)));
    if let Ok(directory) = std::env::var("OMOIKANE_PRINT_ARTIFACTS") {
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(format!("{directory}/page-1.png"), page.encode_png()).unwrap();
    }
}

const MARKER: &[u8] = include_bytes!("fixtures/print/margin-marker.png");

#[test]
fn printed_margin_images_keep_mixed_content_order_and_resolve_relative_urls() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "relative image was not requested"
                    );
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("image test server: {error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut request = [0; 1024];
        let size = stream.read(&mut request).unwrap();
        assert!(
            String::from_utf8_lossy(&request[..size]).starts_with("GET /print/margin-marker.png "),
            "unexpected relative image request"
        );
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            MARKER.len()
        );
        stream.write_all(header.as_bytes()).unwrap();
        stream.write_all(MARKER).unwrap();
    });
    let document =
        TreeBuilder::parse(include_str!("fixtures/print/page-margin-images.html")).document();
    let base_url = format!("http://{address}/print/page-margin-images.html")
        .parse()
        .unwrap();
    let pages = render_document_pages_with_url(
        &document,
        Rect {
            width: 200.0,
            height: 200.0,
            ..Rect::default()
        },
        Some(&base_url),
    )
    .unwrap();
    server.join().unwrap();
    assert_eq!(pages.len(), 1);
    let page = &pages[0];
    if let Ok(directory) = std::env::var("OMOIKANE_PRINT_ARTIFACTS") {
        let image_dir = format!("{directory}/images");
        std::fs::create_dir_all(&image_dir).unwrap();
        std::fs::write(format!("{image_dir}/page-1.png"), page.encode_png()).unwrap();
    }
    assert!(
        (40..80).any(|x| (0..40).any(|y| page.pixel(x, y) == Some(Color::rgb(255, 0, 0)))),
        "image-only min-content box should use the image intrinsic width"
    );
    let mut red_x = Vec::new();
    let mut blue_x = Vec::new();
    let mut black_x = Vec::new();
    for y in 0..40 {
        for x in 80..160 {
            match page.pixel(x, y) {
                Some(Color {
                    r: 255, g: 0, b: 0, ..
                }) => red_x.push(x),
                Some(Color {
                    r: 0, g: 0, b: 255, ..
                }) => blue_x.push(x),
                Some(Color { r, g, b, .. }) if r < 100 && g < 100 && b < 100 => black_x.push(x),
                _ => {}
            }
        }
    }
    assert!(!red_x.is_empty(), "data URL image should paint");
    assert!(!blue_x.is_empty(), "relative URL image should paint");
    let first_red = *red_x.iter().min().unwrap();
    let last_red = *red_x.iter().max().unwrap();
    let first_blue = *blue_x.iter().min().unwrap();
    assert!(
        black_x.iter().any(|&x| x < first_red),
        "text before first image"
    );
    assert!(
        black_x.iter().any(|&x| x > last_red && x < first_blue),
        "text between ordered images"
    );
    assert!(first_red < first_blue);
    assert!(
        (160..200).any(|y| {
            (100..160).any(|x| {
                page.pixel(x, y)
                    .is_some_and(|color| color.r < 100 && color.g < 100 && color.b < 100)
            })
        }),
        "broken image should not suppress adjacent footer text"
    );
}
