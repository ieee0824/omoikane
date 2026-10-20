use super::*;
use crate::html::TreeBuilder;
use crate::paint::{Canvas, Color};

const CARD_COUNT: usize = 48;
const VIEWPORT_WIDTH: u32 = 640;
const VIEWPORT_HEIGHT: u32 = 380;

fn workload_html(include_shadows: bool) -> String {
    let mut cards = String::new();
    for index in 0..CARD_COUNT {
        let column = index % 8;
        let row = index / 8;
        let width = 42 + index;
        let height = 22 + index % 5;
        let radius = 2 + index % 11;
        let blur = 4 + index % 9;
        let spread = index as i32 % 3 - 1;
        let shadow = include_shadows.then(|| {
            format!(
                "box-shadow:2px 1px {blur}px {spread}px rgba({},{},{},{}%);",
                24 + index % 80,
                48 + index % 96,
                72 + index % 112,
                35 + index % 45,
            )
        });
        cards.push_str(&format!(
            "<div class='card' style='left:{}px;top:{}px;width:{width}px;height:{height}px;\
             border-radius:{radius}px;{}'></div>",
            8 + column * 76,
            8 + row * 60,
            shadow.as_deref().unwrap_or_default(),
        ));
    }
    format!(
        "<html><head><style>html,body{{margin:0;width:100%;height:100%;background:white}}\
         .card{{position:absolute;background:#e8f0f8}}</style></head><body>{cards}</body></html>"
    )
}

fn workload_runtime(include_shadows: bool) -> JsRuntime {
    let document = TreeBuilder::parse(&workload_html(include_shadows)).document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime.set_viewport(VIEWPORT_WIDTH as f32, VIEWPORT_HEIGHT as f32);
    runtime
}

fn assert_distinct_public_geometry(runtime: &mut JsRuntime) {
    let script = format!(
        "(() => {{const cards=document.querySelectorAll('.card');const seen={{}};\
         for(let i=0;i<cards.length;i++){{const width=cards[i].getBoundingClientRect().width;\
         if(seen[width])return false;seen[width]=true}}\
         return cards.length==={CARD_COUNT}&&Object.keys(seen).length==={CARD_COUNT}}})()"
    );
    assert!(runtime.eval(&script).unwrap().to_boolean());
}

fn rgba_checksum(canvas: &Canvas) -> u64 {
    canvas
        .pixels()
        .iter()
        .fold(0xcbf29ce484222325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        })
}

fn assert_public_output(canvas: &Canvas) {
    assert_eq!(canvas.width(), VIEWPORT_WIDTH);
    assert_eq!(canvas.height(), VIEWPORT_HEIGHT);
    assert_eq!(canvas.pixel(20, 16), Some(Color::rgb(232, 240, 248)));
    assert_eq!(canvas.pixel(2, 378), Some(Color::rgb(255, 255, 255)));
    assert!(canvas.pixels().chunks_exact(4).all(|pixel| pixel[3] == 255));
}

#[test]
fn static_document_shadow_pixels_are_identical_across_frames() {
    let mut runtime = workload_runtime(true);
    assert_distinct_public_geometry(&mut runtime);

    let first = runtime.paint_current_document().unwrap();
    assert_public_output(&first);
    let expected_pixels = first.pixels().to_vec();
    let expected_checksum = rgba_checksum(&first);

    for _ in 0..3 {
        let canvas = runtime.paint_current_document().unwrap();
        assert_public_output(&canvas);
        assert_eq!(canvas.pixels(), expected_pixels.as_slice());
        assert_eq!(rgba_checksum(&canvas), expected_checksum);
    }

    let mut control = workload_runtime(false);
    let without_shadows = control.paint_current_document().unwrap();
    assert_public_output(&without_shadows);
    assert_ne!(without_shadows.pixels(), expected_pixels.as_slice());
    assert_ne!(rgba_checksum(&without_shadows), expected_checksum);
}

#[test]
#[ignore = "deterministic persistent-cache A/B fixture; no timing threshold"]
fn profile_static_document_shadow_frames() {
    use std::time::{Duration, Instant};

    const MEASURED_FRAMES: usize = 20;

    let mut runtime = workload_runtime(true);
    assert_distinct_public_geometry(&mut runtime);
    let warmup = runtime.paint_current_document().unwrap();
    assert_public_output(&warmup);
    let expected_pixels = warmup.pixels().to_vec();
    let expected_checksum = rgba_checksum(&warmup);

    let mut paint_elapsed = Duration::default();
    let mut folded_checksum = 0u64;
    for frame in 0..MEASURED_FRAMES {
        let started = Instant::now();
        let canvas = runtime.paint_current_document().unwrap();
        paint_elapsed += started.elapsed();

        assert_public_output(&canvas);
        assert_eq!(canvas.pixels(), expected_pixels.as_slice());
        let checksum = rgba_checksum(&canvas);
        assert_eq!(checksum, expected_checksum);
        folded_checksum = folded_checksum.rotate_left(1) ^ checksum ^ frame as u64;
        std::hint::black_box(canvas);
    }

    eprintln!(
        "DOCUMENT_SHADOW_MASK_PROFILE cards={CARD_COUNT} warmups=1 \
         measured_frames={MEASURED_FRAMES} paint_ms={} expected_checksum={expected_checksum} \
         folded_checksum={folded_checksum}",
        paint_elapsed.as_secs_f64() * 1000.0,
    );
}
