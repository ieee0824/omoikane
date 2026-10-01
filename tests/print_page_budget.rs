use omoikane::html::TreeBuilder;
use omoikane::layout::Rect;
use omoikane::paint::{
    Canvas, Color, Image, PaintError, render_document_pages, render_document_pages_with_url,
};

fn render(source: &str) -> Result<Vec<Canvas>, PaintError> {
    render_document_pages(
        &TreeBuilder::parse(source).document(),
        Rect {
            width: 100.0,
            height: 100.0,
            ..Rect::default()
        },
    )
}

#[test]
fn print_page_rejects_excessive_dimension_without_large_allocation() {
    // Only 64 KiB on the unprotected path: safely demonstrate the missing cap.
    assert_eq!(
        render("<style>@page { size: 16385px 1px; margin: 0 }</style><body></body>"),
        Err(PaintError::PrintCanvasBudgetExceeded)
    );
}

#[test]
fn print_page_rejects_excessive_area_and_unrepresentable_css_dimensions() {
    for size in ["8193px 8192px", "1px 16385px", "10000000000px 1px"] {
        assert_eq!(
            render(&format!(
                "<style>@page {{ size: {size}; margin: 0 }}</style><body></body>"
            )),
            Err(PaintError::PrintCanvasBudgetExceeded),
            "{size}"
        );
    }
}

#[test]
fn print_document_checks_total_before_allocating_individually_valid_pages() {
    assert_eq!(
        render(
            "<style>@page { size: 8192px 4096px; margin: 0 }
             body { margin: 0 } div { height: 1px } div + div { break-before: page }
             </style><body><div></div><div></div><div></div></body>"
        ),
        Err(PaintError::PrintCanvasBudgetExceeded)
    );
}

#[test]
fn print_document_checks_named_and_blank_page_sizes() {
    for rule in [
        "@page large { size: 16385px 1px } div + div { page: large }",
        "@page :blank { size: 16385px 1px } div + div { break-before: right }",
    ] {
        assert_eq!(
            render(&format!(
                "<style>@page {{ size: 100px 100px; margin: 0 }}
                 body {{ margin: 0 }} div {{ height: 1px }} {rule}
                 </style><body><div></div><div></div></body>"
            )),
            Err(PaintError::PrintCanvasBudgetExceeded),
            "{rule}"
        );
    }
}

#[test]
fn print_page_preserves_a4_dimensions_and_png_pixels() {
    for (orientation, expected) in [
        ("upright", (794, 1123)),
        ("rotate-left", (1123, 794)),
        ("rotate-right", (1123, 794)),
    ] {
        let pages = render(&format!(
            "<style>@page {{ size: A4; margin: 0; background-color: red;
             page-orientation: {orientation} }}</style><body></body>"
        ))
        .unwrap();
        assert_eq!(pages.len(), 1);
        let page = &pages[0];
        assert_eq!((page.width(), page.height()), expected);
        assert_eq!(
            page.pixels().len(),
            expected.0 as usize * expected.1 as usize * 4
        );
        assert_eq!(page.pixel(0, 0), Some(Color::rgb(255, 0, 0)));
        let decoded = Image::decode_png(&page.encode_png()).unwrap();
        assert_eq!((decoded.width(), decoded.height()), expected);
        assert_eq!(decoded.pixels(), page.pixels());
    }
}

#[test]
fn print_page_with_url_checks_default_dimensions() {
    let document = TreeBuilder::parse("<body></body>").document();
    let base_url: omoikane::http::Url = "https://example.test/".parse().unwrap();
    for (width, height, expected) in [
        (0.0, 100.0, PaintError::InvalidImageBuffer),
        (100.0, f32::NAN, PaintError::InvalidImageBuffer),
        (f32::INFINITY, 100.0, PaintError::InvalidImageBuffer),
        (f32::MAX, 100.0, PaintError::PrintCanvasBudgetExceeded),
        (100.0, 16_384.5, PaintError::PrintCanvasBudgetExceeded),
    ] {
        assert_eq!(
            render_document_pages_with_url(
                &document,
                Rect {
                    width,
                    height,
                    ..Rect::default()
                },
                Some(&base_url)
            ),
            Err(expected)
        );
    }
}
