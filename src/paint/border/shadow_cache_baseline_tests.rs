use super::*;

const CARD_COUNT: usize = 128;

fn workload_document(distinct_geometry: bool) -> crate::dom::NodeHandle {
    let mut cards = String::new();
    for index in 0..CARD_COUNT {
        let column = index % 8;
        let row = index / 8;
        let (width, radius, blur, spread) = if distinct_geometry {
            (
                16 + index % 13,
                index % 7,
                2 + index % 6,
                (index % 3) as i32 - 1,
            )
        } else {
            (20, 4, 6, 0)
        };
        cards.push_str(&format!(
            "<div class='card' style='left:{}px;top:{}px;width:{width}px;\
             border-radius:{radius}px;box-shadow:2px 1px {blur}px {spread}px \
             rgba({},{},{},{}%)'></div>",
            8 + column * 36,
            8 + row * 24,
            20 + index % 80,
            40 + index % 70,
            80 + index % 90,
            35 + index % 60,
        ));
    }
    crate::html::TreeBuilder::parse(&format!("<html><body>{cards}</body></html>")).document()
}

fn workload_layout(
    distinct_geometry: bool,
) -> (crate::layout::LayoutBox, crate::css::StyleResolver, Rect) {
    let document = workload_document(distinct_geometry);
    let mut resolver = crate::css::StyleResolver::new();
    resolver.add_stylesheet(
        crate::css::Origin::Author,
        crate::css::parse_stylesheet(
            "html,body{margin:0;background:white}.card{position:absolute;height:14px;background:white}",
        )
        .unwrap(),
    );
    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 304.0,
        height: 400.0,
    };
    let root = document.query_selector("html").unwrap();
    let layout = crate::layout::layout_tree(&root, &mut resolver, viewport).unwrap();
    (layout, resolver, viewport)
}

fn render_workload(distinct_geometry: bool) -> Canvas {
    let (layout, mut resolver, viewport) = workload_layout(distinct_geometry);
    super::super::paint_layout_with_fonts(&layout, &mut resolver, viewport, Vec::new())
}

fn rgba_checksum(canvas: &Canvas) -> u64 {
    canvas
        .pixels()
        .iter()
        .enumerate()
        .fold(0u64, |sum, (index, &byte)| {
            sum.wrapping_add((index as u64 + 1).wrapping_mul(u64::from(byte) + 1))
        })
}

#[test]
fn repeated_and_distinct_shadow_workloads_are_rgba_deterministic() {
    let repeated = render_workload(false);
    let repeated_again = render_workload(false);
    assert_eq!(repeated.pixels(), repeated_again.pixels());

    let distinct = render_workload(true);
    let distinct_again = render_workload(true);
    assert_eq!(distinct.pixels(), distinct_again.pixels());
    assert_ne!(repeated.pixels(), distinct.pixels());
    assert!(repeated.pixels().chunks_exact(4).any(|pixel| pixel[3] > 0));
    assert!(distinct.pixels().chunks_exact(4).any(|pixel| pixel[3] > 0));
}

fn profile_workload(distinct_geometry: bool) -> (f64, u64) {
    let (layout, mut resolver, viewport) = workload_layout(distinct_geometry);
    let reference =
        super::super::paint_layout_with_fonts(&layout, &mut resolver, viewport, Vec::new());
    let expected = reference.pixels().to_vec();
    let started = std::time::Instant::now();
    let mut checksum = 0u64;
    for _ in 0..12 {
        let canvas =
            super::super::paint_layout_with_fonts(&layout, &mut resolver, viewport, Vec::new());
        assert_eq!(canvas.pixels(), expected);
        checksum = checksum.wrapping_add(rgba_checksum(&canvas));
        std::hint::black_box(canvas);
    }
    (started.elapsed().as_secs_f64() * 1000.0, checksum)
}

#[test]
#[ignore = "deterministic A/B fixture; no timing threshold"]
fn profile_repeated_and_distinct_frame_shadow_masks() {
    let (repeated_ms, repeated_checksum) = profile_workload(false);
    let (distinct_ms, distinct_checksum) = profile_workload(true);
    eprintln!(
        "FRAME_SHADOW_MASK_PROFILE cards={CARD_COUNT} frames=12 \
         repeated_ms={repeated_ms} repeated_checksum={repeated_checksum} \
         distinct_ms={distinct_ms} distinct_checksum={distinct_checksum}"
    );
}
