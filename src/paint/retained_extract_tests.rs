use crate::css::{Origin, StyleResolver};
use crate::dom::NodeHandle;
use crate::html::TreeBuilder;
use crate::layout::{LayoutBox, Rect, layout_tree};

use super::retained_extract::{
    RetainedPaintError, RetainedPaintFrame, RetainedPaintOutcome, RetainedPaintSpace,
    paint_layout_with_injected_collection_failure, paint_layout_with_retained_fallback,
};
use super::{
    Canvas, Color, Image, apply_scroll_offsets, extract_author_stylesheets,
    paint_layout_with_web_fonts, parse_stylesheet_forgiving,
};

fn assert_live_paint_inputs(unsupported: bool) {
    let document = NodeHandle::document();
    let body = NodeHandle::element("body");
    let link = NodeHandle::element("a");
    link.set_attribute("href", "/visited");
    let iframe = NodeHandle::element("iframe");
    let pin = NodeHandle::element("div");
    pin.set_attribute("id", "pin");
    document.append_child(body.clone());
    body.append_child(link.clone());
    body.append_child(iframe.clone());
    body.append_child(pin);
    if unsupported {
        body.set_attribute("style", "position:sticky;top:0");
    }
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet_forgiving(
            "*{margin:0;padding:0}body{height:100px}\
         a:link{display:block;width:8px;height:8px;background:red}\
         a:visited{background:blue}\
         iframe{display:block;width:8px;height:8px;border:0}\
         #pin{position:fixed;left:16px;top:0;width:8px;height:8px;background:lime}",
        ),
    );
    let layout = layout_tree(&document, &mut resolver, viewport()).unwrap();
    let snapshots = std::collections::HashMap::from([(
        iframe.identity(),
        Image::new(1, 1, vec![255, 255, 0, 255]).unwrap(),
    )]);
    let shadow_masks = super::border::FrameShadowMaskCache::new(false);
    let expected = super::paint_layout_with_document_snapshots_cached(
        &layout,
        &mut resolver,
        viewport(),
        Vec::new(),
        None,
        [link.identity()],
        &snapshots,
        false,
        &shadow_masks,
    )
    .0;
    let outcome = super::retained_extract::paint_layout_with_retained_document_snapshots(
        &layout,
        &mut resolver,
        viewport(),
        &[],
        None,
        &[link.identity()],
        &snapshots,
        &shadow_masks,
    );
    match (&outcome, unsupported) {
        (RetainedPaintOutcome::Extracted(_), false) => {}
        (
            RetainedPaintOutcome::Fallback {
                reason: RetainedPaintError::StickyPosition,
                ..
            },
            true,
        ) => {}
        _ => panic!("unexpected live-input extraction disposition"),
    }
    let actual = outcome.into_canvas().unwrap();
    assert_same_canvas(&actual, &expected);
    assert_eq!(actual.pixel(1, 1), Some(Color::rgb(0, 0, 255)));
    assert_eq!(actual.pixel(1, 9), Some(Color::rgb(255, 255, 0)));
    assert_eq!(actual.pixel(17, 1), Some(Color::rgb(0, 255, 0)));

    // Private visited paint state must be cleared on both success and fallback.
    let unvisited = super::paint_layout_with_document_snapshots(
        &layout,
        &mut resolver,
        viewport(),
        Vec::new(),
        None,
        [],
        &snapshots,
    );
    assert_eq!(unvisited.pixel(1, 1), Some(Color::rgb(255, 0, 0)));
    assert_eq!(unvisited.pixel(1, 9), Some(Color::rgb(255, 255, 0)));
}

#[test]
fn live_child_snapshots_and_visited_colors_survive_extraction() {
    assert_live_paint_inputs(false);
}

#[test]
fn live_child_snapshots_and_visited_colors_survive_ordinary_fallback() {
    assert_live_paint_inputs(true);
}

fn viewport() -> Rect {
    Rect {
        x: 0.0,
        y: 0.0,
        width: 32.0,
        height: 32.0,
    }
}

fn build_fixture(
    html: &str,
    window_scroll: (f32, f32),
    setup: impl FnOnce(&NodeHandle),
) -> (LayoutBox, StyleResolver) {
    let document = TreeBuilder::parse(html).document();
    setup(&document);
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }
    let mut layout = layout_tree(&document, &mut resolver, viewport()).unwrap();
    apply_scroll_offsets(&mut layout, &mut resolver, viewport(), window_scroll);
    (layout, resolver)
}

fn ordinary_fixture(
    html: &str,
    window_scroll: (f32, f32),
    setup: impl FnOnce(&NodeHandle),
) -> Canvas {
    let (layout, mut resolver) = build_fixture(html, window_scroll, setup);
    paint_layout_with_web_fonts(&layout, &mut resolver, viewport(), Vec::new(), None)
}

fn retained_fixture(
    html: &str,
    window_scroll: (f32, f32),
    setup: impl FnOnce(&NodeHandle),
) -> RetainedPaintOutcome {
    let (layout, mut resolver) = build_fixture(html, window_scroll, setup);
    paint_layout_with_retained_fallback(&layout, &mut resolver, viewport(), &[], None)
}

fn assert_same_canvas(actual: &Canvas, expected: &Canvas) {
    assert_eq!(
        (actual.width(), actual.height()),
        (expected.width(), expected.height())
    );
    assert_eq!(actual.pixels(), expected.pixels());
}

fn expect_extracted(outcome: RetainedPaintOutcome) -> RetainedPaintFrame {
    match outcome {
        RetainedPaintOutcome::Extracted(frame) => frame,
        RetainedPaintOutcome::Fallback { reason, .. } => {
            panic!("expected retained frame, got {reason:?}")
        }
        RetainedPaintOutcome::Rejected { reason } => {
            panic!("expected retained frame, request was rejected: {reason:?}")
        }
    }
}

#[test]
fn invalid_or_oversized_viewports_are_rejected_before_canvas_allocation() {
    let html = "<style>html,body{margin:0;background:white}</style>";
    let (layout, mut resolver) = build_fixture(html, (0.0, 0.0), |_| {});
    for (viewport, expected) in [
        (
            Rect {
                x: f32::NAN,
                ..viewport()
            },
            RetainedPaintError::InvalidViewport,
        ),
        (
            Rect {
                width: f32::INFINITY,
                ..viewport()
            },
            RetainedPaintError::InvalidViewport,
        ),
        (
            Rect {
                width: 8192.0,
                height: 8192.0,
                ..viewport()
            },
            RetainedPaintError::Capacity,
        ),
    ] {
        match paint_layout_with_retained_fallback(&layout, &mut resolver, viewport, &[], None) {
            RetainedPaintOutcome::Rejected { reason } => assert_eq!(reason, expected),
            RetainedPaintOutcome::Fallback { reason, .. } => {
                panic!("unsafe viewport unexpectedly entered ordinary fallback: {reason:?}")
            }
            RetainedPaintOutcome::Extracted(_) => {
                panic!("unsafe viewport unexpectedly allocated an extracted frame")
            }
        }
    }
}

#[test]
fn too_many_fixed_roots_use_bounded_ordinary_fallback() {
    let mut html = String::from(
        "<style>*{margin:0;padding:0}.fixed{position:fixed;width:1px;height:1px}</style>",
    );
    for _ in 0..32 {
        html.push_str("<div class=fixed></div>");
    }
    let ordinary = ordinary_fixture(&html, (0.0, 0.0), |_| {});
    match retained_fixture(&html, (0.0, 0.0), |_| {}) {
        RetainedPaintOutcome::Fallback {
            canvas,
            reason: RetainedPaintError::Capacity,
            discarded_runs: 0,
        } => assert_same_canvas(&canvas, &ordinary),
        RetainedPaintOutcome::Fallback { reason, .. } => {
            panic!("unexpected fixed-root capacity result: {reason:?}")
        }
        RetainedPaintOutcome::Extracted(_) => {
            panic!("32 fixed roots exceed the bounded retained-run budget")
        }
        RetainedPaintOutcome::Rejected { reason } => {
            panic!("bounded fixture was rejected: {reason:?}")
        }
    }
}

fn assert_supported(
    html: &str,
    window_scroll: (f32, f32),
    spaces: &[RetainedPaintSpace],
) -> (Canvas, RetainedPaintFrame) {
    let ordinary = ordinary_fixture(html, window_scroll, |_| {});
    let frame = expect_extracted(retained_fixture(html, window_scroll, |_| {}));
    assert_eq!(
        frame.runs().iter().map(|run| run.space).collect::<Vec<_>>(),
        spaces
    );
    assert_same_canvas(&frame.compose(), &ordinary);
    (ordinary, frame)
}

fn exact_color_bounds(canvas: &Canvas, color: Color) -> Option<(u32, u32, u32, u32)> {
    let mut bounds: Option<(u32, u32, u32, u32)> = None;
    for y in 0..canvas.height() {
        for x in 0..canvas.width() {
            if canvas.pixel(x, y) != Some(color) {
                continue;
            }
            bounds = Some(match bounds {
                None => (x, y, x, y),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            });
        }
    }
    bounds
}

fn image_has_exact_color(image: &Image, color: Color) -> bool {
    image
        .pixels()
        .chunks_exact(4)
        .any(|pixel| pixel == [color.r, color.g, color.b, color.a])
}

#[test]
fn empty_and_non_fixed_documents_emit_one_document_run() {
    let empty = "<style>html,body{margin:0;background:transparent}</style><body></body>";
    let (empty_canvas, empty_frame) =
        assert_supported(empty, (0.0, 0.0), &[RetainedPaintSpace::Document]);
    assert!(empty_canvas.pixels().iter().all(|channel| *channel == 0));
    assert_eq!(empty_frame.runs().len(), 1);

    let ordinary = "<style>*{margin:0}html{background:white}body{height:80px}\
        #a{width:12px;height:12px;background:red}\
        #b{position:absolute;left:12px;top:20px;width:12px;height:12px;background:blue}\
        </style><div id=a></div><div id=b></div>";
    for scroll in [0.0, 12.0] {
        assert_supported(ordinary, (0.0, scroll), &[RetainedPaintSpace::Document]);
    }
}

#[test]
fn extracted_runs_preserve_content_fixed_content_order() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:100px}\
        #early{position:absolute;left:0;top:20px;width:32px;height:24px;background:red}\
        #pin{position:fixed;left:4px;top:4px;width:24px;height:24px;background:blue}\
        #late{position:absolute;left:8px;top:28px;width:16px;height:16px;background:lime}\
        </style><div id=early></div><div id=pin></div><div id=late></div>";
    let (canvas, _) = assert_supported(
        html,
        (0.0, 20.0),
        &[
            RetainedPaintSpace::Document,
            RetainedPaintSpace::ViewportFixed,
            RetainedPaintSpace::Document,
        ],
    );
    assert_eq!(canvas.pixel(1, 1), Some(Color::rgb(255, 0, 0)));
    assert_eq!(canvas.pixel(5, 5), Some(Color::rgb(0, 0, 255)));
    assert_eq!(canvas.pixel(9, 9), Some(Color::rgb(0, 255, 0)));
    assert_eq!(canvas.pixel(30, 30), Some(Color::rgb(255, 255, 255)));
}

#[test]
fn extracted_runs_preserve_negative_and_positive_fixed_phases() {
    let html = "<style>*{margin:0;padding:0}html{background:white}\
        #positive{position:fixed;left:16px;top:0;width:16px;height:16px;z-index:2;background:lime}\
        #content{width:24px;height:16px;margin-left:8px;background:red}\
        #negative{position:fixed;left:0;top:0;width:16px;height:16px;z-index:-2;background:blue}\
        </style><div id=positive></div><div id=content></div><div id=negative></div>";
    let (canvas, _) = assert_supported(
        html,
        (0.0, 0.0),
        &[
            RetainedPaintSpace::Document,
            RetainedPaintSpace::ViewportFixed,
            RetainedPaintSpace::Document,
            RetainedPaintSpace::ViewportFixed,
            RetainedPaintSpace::Document,
        ],
    );
    assert_eq!(canvas.pixel(4, 4), Some(Color::rgb(0, 0, 255)));
    assert_eq!(canvas.pixel(12, 4), Some(Color::rgb(255, 0, 0)));
    assert_eq!(canvas.pixel(20, 4), Some(Color::rgb(0, 255, 0)));
}

#[test]
fn fixed_under_transform_stays_in_document_run() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #host{position:absolute;left:4px;top:20px;width:24px;height:30px;\
              transform:translateX(3px);background:#ffcc00}\
        #contained{position:fixed;left:5px;top:5px;width:8px;height:8px;background:red}\
        #viewport{position:fixed;right:2px;top:2px;width:6px;height:6px;background:blue}\
        </style><div id=host><div id=contained></div></div><div id=viewport></div>";
    let spaces = [
        RetainedPaintSpace::Document,
        RetainedPaintSpace::ViewportFixed,
        RetainedPaintSpace::Document,
    ];
    let (before, before_frame) = assert_supported(html, (0.0, 0.0), &spaces);
    let (after, after_frame) = assert_supported(html, (0.0, 10.0), &spaces);
    let before_red = exact_color_bounds(&before, Color::rgb(255, 0, 0)).unwrap();
    let after_red = exact_color_bounds(&after, Color::rgb(255, 0, 0)).unwrap();
    assert_eq!(before_red.1.saturating_sub(after_red.1), 10);
    assert_eq!(
        exact_color_bounds(&before, Color::rgb(0, 0, 255)),
        exact_color_bounds(&after, Color::rgb(0, 0, 255))
    );
    let before_fixed = &before_frame.runs()[1].image;
    let after_fixed = &after_frame.runs()[1].image;
    assert_eq!(before_fixed, after_fixed);
    assert!(image_has_exact_color(before_fixed, Color::rgb(0, 0, 255)));
    assert!(!image_has_exact_color(before_fixed, Color::rgb(255, 0, 0)));
    assert!(
        before_frame
            .runs()
            .iter()
            .filter(|run| run.space == RetainedPaintSpace::Document)
            .any(|run| image_has_exact_color(&run.image, Color::rgb(255, 0, 0)))
    );
}

#[test]
fn cross_space_effects_and_clips_fall_back_without_partial_runs() {
    for (declaration, expected) in [
        ("opacity:.5", RetainedPaintError::CrossSpaceEffect),
        (
            "filter:brightness(.5)",
            RetainedPaintError::CrossSpaceEffect,
        ),
        (
            "mask-image:linear-gradient(90deg,black 50%,transparent 50%)",
            RetainedPaintError::CrossSpaceEffect,
        ),
        ("overflow:hidden", RetainedPaintError::CrossSpaceClip),
        ("clip-path:inset(0 2px)", RetainedPaintError::CrossSpaceClip),
    ] {
        let html = format!(
            "<style>*{{margin:0;padding:0}}html{{background:white}}\
             #ancestor{{position:relative;width:24px;height:24px;{declaration}}}\
             #ordinary{{width:12px;height:12px;background:red}}\
             #fixed{{position:fixed;left:20px;top:4px;width:12px;height:12px;background:blue}}\
             </style><div id=ancestor><div id=ordinary></div><div id=fixed></div></div>"
        );
        let ordinary = ordinary_fixture(&html, (0.0, 0.0), |_| {});
        match expected {
            RetainedPaintError::CrossSpaceClip => {
                assert_eq!(ordinary.pixel(21, 8), Some(Color::rgb(0, 0, 255)));
                assert_eq!(ordinary.pixel(28, 8), Some(Color::rgb(255, 255, 255)));
            }
            RetainedPaintError::CrossSpaceEffect => {
                assert_ne!(ordinary.pixel(21, 8), Some(Color::rgb(0, 0, 255)));
            }
            _ => unreachable!(),
        }
        match retained_fixture(&html, (0.0, 0.0), |_| {}) {
            RetainedPaintOutcome::Fallback {
                canvas,
                reason,
                discarded_runs,
            } => {
                assert_eq!(reason, expected, "{declaration}");
                assert_eq!(discarded_runs, 0, "{declaration}");
                assert_same_canvas(&canvas, &ordinary);
            }
            RetainedPaintOutcome::Extracted(_) => {
                panic!("{declaration} must not publish a retained frame")
            }
            RetainedPaintOutcome::Rejected { reason } => {
                panic!("bounded fixture was rejected: {reason:?}")
            }
        }
    }
}

#[test]
fn generated_inline_fixed_background_uses_ordinary_fallback() {
    let html = "<style>*{margin:0;padding:0}html{background:white}\
        span::before{content:'';width:8px;height:8px;\
        background-image:linear-gradient(red,red);background-attachment:fixed}\
        </style><span>body</span>";
    let ordinary = ordinary_fixture(html, (0.0, 0.0), |_| {});
    match retained_fixture(html, (0.0, 0.0), |_| {}) {
        RetainedPaintOutcome::Fallback {
            canvas,
            reason: RetainedPaintError::FixedBackground,
            discarded_runs: 0,
        } => assert_same_canvas(&canvas, &ordinary),
        RetainedPaintOutcome::Fallback { reason, .. } => {
            panic!("unexpected generated-inline fallback: {reason:?}")
        }
        RetainedPaintOutcome::Extracted(_) => {
            panic!("generated inline fixed background must not be retained")
        }
        RetainedPaintOutcome::Rejected { reason } => {
            panic!("bounded fixture was rejected: {reason:?}")
        }
    }
}

#[test]
fn viewport_dependent_non_fixed_features_fall_back() {
    for (declaration, expected) in [
        ("position:sticky;top:0", RetainedPaintError::StickyPosition),
        (
            "background:linear-gradient(red,red);background-attachment:fixed",
            RetainedPaintError::FixedBackground,
        ),
        (
            "backdrop-filter:brightness(.5)",
            RetainedPaintError::BackdropFilter,
        ),
    ] {
        let html = format!(
            "<style>*{{margin:0;padding:0}}html{{background:white}}body{{height:80px}}\
             #target{{width:20px;height:20px;{declaration}}}\
             </style><div id=target></div>"
        );
        let ordinary = ordinary_fixture(&html, (0.0, 8.0), |_| {});
        match retained_fixture(&html, (0.0, 8.0), |_| {}) {
            RetainedPaintOutcome::Fallback { canvas, reason, .. } => {
                assert_eq!(reason, expected, "{declaration}");
                assert_same_canvas(&canvas, &ordinary);
            }
            RetainedPaintOutcome::Extracted(_) => panic!("{declaration} must fall back"),
            RetainedPaintOutcome::Rejected { reason } => {
                panic!("bounded fixture was rejected: {reason:?}")
            }
        }
    }
}

#[test]
fn top_layer_falls_back_to_the_ordinary_painter() {
    let html = "<style>*{margin:0}html{background:white}\
        #top{position:fixed;left:4px;top:4px;width:16px;height:16px;background:red}\
        </style><div id=top popover></div>";
    let setup = |document: &NodeHandle| {
        let _ = document
            .query_selector("#top")
            .unwrap()
            .set_popover_open(true);
    };
    let ordinary = ordinary_fixture(html, (0.0, 0.0), setup);
    match retained_fixture(html, (0.0, 0.0), setup) {
        RetainedPaintOutcome::Fallback {
            canvas,
            reason: RetainedPaintError::TopLayer,
            discarded_runs: 0,
        } => assert_same_canvas(&canvas, &ordinary),
        RetainedPaintOutcome::Fallback { reason, .. } => {
            panic!("unexpected top-layer rejection: {reason:?}")
        }
        RetainedPaintOutcome::Extracted(_) => panic!("top layer must fall back"),
        RetainedPaintOutcome::Rejected { reason } => {
            panic!("bounded fixture was rejected: {reason:?}")
        }
    }
}

#[test]
fn late_collection_failure_discards_every_partial_run() {
    let html = "<style>*{margin:0;padding:0}html{background:transparent}\
        #a{position:absolute;left:0;top:0;width:4px;height:4px;background:rgba(255,0,0,.5)}\
        #fixed-a{position:fixed;left:6px;top:0;width:4px;height:4px;background:rgba(0,0,255,.5)}\
        #b{position:absolute;left:12px;top:0;width:4px;height:4px;background:rgba(0,255,0,.5)}\
        #fixed-b{position:fixed;left:18px;top:0;width:4px;height:4px;background:rgba(255,0,255,.5)}\
        #c{position:absolute;left:24px;top:0;width:4px;height:4px;background:rgba(0,0,0,.5)}\
        </style><div id=a></div><div id=fixed-a></div><div id=b></div>\
        <div id=fixed-b></div><div id=c></div>";
    let ordinary = ordinary_fixture(html, (0.0, 0.0), |_| {});
    assert_eq!(ordinary.pixel(1, 1), Some(Color::rgba(255, 0, 0, 128)));
    let (layout, mut resolver) = build_fixture(html, (0.0, 0.0), |_| {});
    match paint_layout_with_injected_collection_failure(&layout, &mut resolver, viewport(), 3) {
        RetainedPaintOutcome::Fallback {
            canvas,
            reason: RetainedPaintError::CollectionFailed,
            discarded_runs: 3,
        } => assert_same_canvas(&canvas, &ordinary),
        RetainedPaintOutcome::Fallback {
            reason,
            discarded_runs,
            ..
        } => panic!("unexpected late failure {reason:?} after {discarded_runs} runs"),
        RetainedPaintOutcome::Extracted(_) => panic!("injected failure must discard the candidate"),
        RetainedPaintOutcome::Rejected { reason } => {
            panic!("bounded fixture was rejected: {reason:?}")
        }
    }

    let frame = expect_extracted(retained_fixture(html, (0.0, 0.0), |_| {}));
    assert_eq!(
        frame.runs().iter().map(|run| run.space).collect::<Vec<_>>(),
        vec![
            RetainedPaintSpace::Document,
            RetainedPaintSpace::ViewportFixed,
            RetainedPaintSpace::Document,
            RetainedPaintSpace::ViewportFixed,
            RetainedPaintSpace::Document,
        ]
    );
    assert_same_canvas(&frame.compose(), &ordinary);
}
