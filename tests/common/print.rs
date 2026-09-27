use omoikane::html::TreeBuilder;
use omoikane::layout::Rect;
use omoikane::paint::{Canvas, Color, render_document_pages};

pub fn render_pages(source: &str, width: f32, height: f32) -> Vec<Canvas> {
    let document = TreeBuilder::parse(source).document();
    render_document_pages(
        &document,
        Rect {
            width,
            height,
            ..Rect::default()
        },
    )
    .unwrap()
}

pub fn color_bounds(
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

pub fn save_pages(pages: &[Canvas], subdirectory: Option<&str>) {
    let Ok(directory) = std::env::var("OMOIKANE_PRINT_ARTIFACTS") else {
        return;
    };
    let directory = std::path::Path::new(&directory);
    let directory =
        subdirectory.map_or_else(|| directory.to_path_buf(), |name| directory.join(name));
    std::fs::create_dir_all(&directory).unwrap();
    for (index, page) in pages.iter().enumerate() {
        std::fs::write(
            directory.join(format!("page-{}.png", index + 1)),
            page.encode_png(),
        )
        .unwrap();
    }
}
