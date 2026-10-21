use crate::css::{Origin, StyleResolver};
use crate::dom::NodeHandle;
use crate::html::TreeBuilder;
use crate::layout::{InlineFragmentContent, LayoutBox, Rect, layout_tree};
use std::sync::Arc;

use super::retained_extract::{
    RetainedOverscanFrame, RetainedOverscanOutcome, RetainedPaintError, RetainedPaintFrame,
    RetainedPaintOutcome, RetainedPaintRunIdentity, RetainedPaintSpace, VerticalOverscanPlan,
    paint_layout_with_injected_collection_failure, paint_layout_with_retained_fallback,
    paint_layout_with_vertical_overscan,
};
use super::{
    Canvas, Color, GifDisposal, Image, ImageAnimation, ImageFrame, apply_scroll_offsets,
    extract_author_stylesheets, paint_layout_with_web_fonts, parse_stylesheet_forgiving,
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
    let (mut layout, mut resolver) = build_raw_fixture(html, setup);
    apply_scroll_offsets(&mut layout, &mut resolver, viewport(), window_scroll);
    (layout, resolver)
}

fn build_raw_fixture(html: &str, setup: impl FnOnce(&NodeHandle)) -> (LayoutBox, StyleResolver) {
    let document = TreeBuilder::parse(html).document();
    setup(&document);
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }
    let layout = layout_tree(&document, &mut resolver, viewport()).unwrap();
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
    assert_eq!(actual.pixels().len(), expected.pixels().len());
    let width = actual.width() as usize;
    let first = actual
        .pixels()
        .chunks_exact(4)
        .zip(expected.pixels().chunks_exact(4))
        .enumerate()
        .find(|(_, (actual, expected))| actual != expected);
    assert!(
        first.is_none(),
        "first differing RGBA pixel: {:?}",
        first.map(|(index, (actual, expected))| (index % width, index / width, actual, expected))
    );
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

fn expect_overscan(outcome: RetainedOverscanOutcome) -> RetainedOverscanFrame {
    match outcome {
        RetainedOverscanOutcome::Extracted(frame) => frame,
        RetainedOverscanOutcome::Fallback { reason, .. } => {
            panic!("expected overscan frame, got {reason:?}")
        }
        RetainedOverscanOutcome::Rejected { reason } => {
            panic!("expected overscan frame, request was rejected: {reason:?}")
        }
    }
}

fn overscan_fixture(
    html: &str,
    plan: VerticalOverscanPlan,
    setup: impl FnOnce(&NodeHandle),
) -> RetainedOverscanOutcome {
    let (layout, mut resolver) = build_raw_fixture(html, setup);
    let shadow_masks = super::border::FrameShadowMaskCache::new(false);
    paint_layout_with_vertical_overscan(
        &layout,
        &mut resolver,
        viewport(),
        plan,
        &[],
        None,
        &[],
        &std::collections::HashMap::new(),
        &shadow_masks,
    )
}

#[test]
fn flat_overscan_reuses_planning_pixels_without_reducing_coverage() {
    let html = "<style>*{margin:0;padding:0}html{background:white}\
        body{height:128px;background:linear-gradient(red,blue)}\
        #overlay{position:absolute;left:3px;top:18px;width:20px;height:58px;\
        background:rgba(0,255,0,.5)}</style><div id=overlay></div>";
    for current_index in 0..3 {
        let before = super::retained_extract::retained_raster_count();
        let frame = expect_overscan(overscan_fixture(
            html,
            VerticalOverscanPlan::new(0, 0, 3, current_index).unwrap(),
            |_| {},
        ));
        assert_eq!(super::retained_extract::retained_raster_count() - before, 3);
        assert_eq!(frame.coverage(), ((0, 0), (0, 64)));
        assert_eq!(frame.current_scroll(), (0, i32::from(current_index) * 32));
        assert_eq!(frame.runs().len(), 1);
        assert_eq!(
            frame.runs()[0]
                .tiles()
                .iter()
                .map(|tile| tile.origin())
                .collect::<Vec<_>>(),
            [(0, 0), (0, 32), (0, 64)]
        );
        for scroll_y in [0, 16, 32, 48, 64] {
            let expected = ordinary_fixture(html, (0.0, scroll_y as f32), |_| {});
            assert_same_canvas(&frame.compose_integer((0, scroll_y)).unwrap(), &expected);
        }
        assert!(matches!(
            frame.compose_integer((0, 65)),
            Err(RetainedPaintError::OutsideOverscanCoverage)
        ));
    }
}

#[test]
fn vertical_overscan_matches_real_painter_between_samples() {
    let before = super::retained_extract::retained_raster_count();
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:96px}\
        .band{position:absolute;left:0;width:32px;height:32px}\
        #a{top:0;background:red}#b{top:32px;background:#ff8000}\
        #c{top:64px;background:#800080}\
        #pin{position:fixed;left:4px;top:4px;width:20px;height:20px;background:blue;\
        filter:brightness(1)}\
        #late{position:absolute;left:16px;top:0;width:8px;height:96px;background:lime}\
        </style><div id=a class=band></div><div id=b class=band></div>\
        <div id=c class=band></div><div id=pin></div><div id=late></div>";
    let plan = VerticalOverscanPlan::new(0, 0, 3, 1).unwrap();
    let frame = expect_overscan(overscan_fixture(html, plan, |_| {}));
    assert_eq!(super::retained_extract::retained_raster_count() - before, 4);
    assert_eq!(frame.coverage(), ((0, 0), (0, 64)));
    assert_eq!(frame.current_scroll(), (0, 32));
    assert_eq!(
        frame
            .runs()
            .iter()
            .map(|run| run.space())
            .collect::<Vec<_>>(),
        [
            RetainedPaintSpace::Document,
            RetainedPaintSpace::ViewportFixed,
            RetainedPaintSpace::Document,
        ]
    );
    assert_eq!(
        frame
            .runs()
            .iter()
            .map(|run| run.identity())
            .collect::<Vec<_>>(),
        [
            RetainedPaintRunIdentity::Document(0),
            RetainedPaintRunIdentity::ViewportFixed(0),
            RetainedPaintRunIdentity::Document(1),
        ]
    );
    assert_eq!(
        frame.runs()[0]
            .tiles()
            .iter()
            .map(|tile| tile.origin())
            .collect::<Vec<_>>(),
        [(0, 0), (0, 32), (0, 64)]
    );
    assert_eq!(frame.runs()[1].tiles().len(), 1);
    assert_eq!(frame.runs()[1].tiles()[0].origin(), (0, 0));
    assert_eq!(frame.runs()[2].tiles().len(), 3);

    for scroll_y in [0, 16, 32, 48, 64] {
        let expected = ordinary_fixture(html, (0.0, scroll_y as f32), |_| {});
        let actual = frame.compose_integer((0, scroll_y)).unwrap();
        assert_same_canvas(&actual, &expected);
        let logical = frame
            .compose_logical((0.0, f64::from(scroll_y)), 1.0)
            .unwrap();
        assert_eq!(logical.pixels(), actual.pixels());
    }
    let at_zero = frame.compose_integer((0, 0)).unwrap();
    assert_eq!(at_zero.pixel(2, 2), Some(Color::rgb(255, 0, 0)));
    assert_eq!(at_zero.pixel(5, 5), Some(Color::rgb(0, 0, 255)));
    assert_eq!(at_zero.pixel(17, 5), Some(Color::rgb(0, 255, 0)));
    let at_48 = frame.compose_integer((0, 48)).unwrap();
    assert_eq!(at_48.pixel(2, 2), Some(Color::rgb(255, 128, 0)));
    assert_eq!(at_48.pixel(2, 20), Some(Color::rgb(128, 0, 128)));
    assert!(matches!(
        frame.compose_integer((0, 65)),
        Err(RetainedPaintError::OutsideOverscanCoverage)
    ));
}

#[test]
fn vertical_overscan_preserves_translucent_run_order_at_tile_boundaries() {
    let html = "<style>*{margin:0;padding:0}html,body{background:transparent}\
        body{height:96px}#under,#over{position:absolute;left:0;top:0;\
        width:32px;height:96px}#under{background:rgba(255,0,0,.5)}\
        #pin{position:fixed;left:0;top:0;width:32px;height:32px;\
        background:rgba(0,0,255,.5)}#over{background:rgba(0,255,0,.25)}\
        </style><div id=under></div><div id=pin></div><div id=over></div>";
    let frame = expect_overscan(overscan_fixture(
        html,
        VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(),
        |_| {},
    ));
    assert_eq!(
        frame
            .runs()
            .iter()
            .map(|run| run.space())
            .collect::<Vec<_>>(),
        [
            RetainedPaintSpace::Document,
            RetainedPaintSpace::ViewportFixed,
            RetainedPaintSpace::Document,
        ]
    );
    for scroll_y in [0, 16, 32, 48, 64] {
        let expected = ordinary_fixture(html, (0.0, scroll_y as f32), |_| {});
        let actual = frame.compose_integer((0, scroll_y)).unwrap();
        assert_same_canvas(&actual, &expected);
    }
    let layered = frame.compose_integer((0, 16)).unwrap().pixel(4, 4).unwrap();
    assert!(layered.a > 0 && layered.a < 255);
}

#[test]
fn overscan_unproven_sticky_scene_repaints_the_current_scroll() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:96px}\
        #sticky{position:sticky;top:8px;width:16px;height:16px;background:red}\
        </style><div id=sticky></div>";
    let expected = ordinary_fixture(html, (0.0, 32.0), |_| {});
    let plan = VerticalOverscanPlan::new(0, 0, 3, 1).unwrap();
    match overscan_fixture(html, plan, |_| {}) {
        RetainedOverscanOutcome::Fallback {
            canvas,
            reason: RetainedPaintError::StickyPosition,
            discarded_tiles: 0,
        } => assert_same_canvas(&canvas, &expected),
        RetainedOverscanOutcome::Fallback { reason, .. } => {
            panic!("unexpected overscan fallback: {reason:?}")
        }
        RetainedOverscanOutcome::Extracted(_) => panic!("unproven sticky overscan must fall back"),
        RetainedOverscanOutcome::Rejected { reason } => {
            panic!("bounded sticky fixture was rejected: {reason:?}")
        }
    }
}

#[test]
fn overscan_nested_effect_filter_keeps_exact_ordinary_fallback() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:96px}\
        #blur{position:absolute;left:4px;top:24px;width:24px;height:24px;\
        background:red;filter:blur(6px)}#wrap{opacity:.8}</style><div id=wrap><div id=blur></div></div>";
    let expected = ordinary_fixture(html, (0.0, 32.0), |_| {});
    let plan = VerticalOverscanPlan::new(0, 0, 3, 1).unwrap();
    match overscan_fixture(html, plan, |_| {}) {
        RetainedOverscanOutcome::Fallback {
            canvas,
            reason: RetainedPaintError::ViewportEdgeFilter,
            discarded_tiles: 0,
        } => assert_same_canvas(&canvas, &expected),
        RetainedOverscanOutcome::Fallback { reason, .. } => {
            panic!("unexpected filter fallback: {reason:?}")
        }
        RetainedOverscanOutcome::Extracted(_) => {
            panic!("document filter must not be stitched across sample edges")
        }
        RetainedOverscanOutcome::Rejected { reason } => {
            panic!("bounded filter fixture was rejected: {reason:?}")
        }
    }
}

#[test]
fn overscan_rejects_invalid_coordinates_and_peak_bytes_before_raster() {
    let before = super::retained_extract::retained_raster_count();
    assert_eq!(
        VerticalOverscanPlan::new(-1, 0, 1, 0),
        Err(RetainedPaintError::InvalidOverscanPlan)
    );
    assert_eq!(
        VerticalOverscanPlan::new(0, 0, 8, 0),
        Err(RetainedPaintError::InvalidOverscanPlan)
    );
    let html = "<style>*{margin:0;padding:0}html{background:white}</style>";
    let (layout, mut resolver) = build_raw_fixture(html, |_| {});
    let shadow_masks = super::border::FrameShadowMaskCache::new(false);
    assert!(matches!(
        paint_layout_with_vertical_overscan(
            &layout,
            &mut resolver,
            viewport(),
            VerticalOverscanPlan::new(1 << 24, 0, 1, 0).unwrap(),
            &[],
            None,
            &[],
            &std::collections::HashMap::new(),
            &shadow_masks,
        ),
        RetainedOverscanOutcome::Rejected {
            reason: RetainedPaintError::InvalidOverscanPlan
        }
    ));
    let oversized = Rect {
        width: 4096.0,
        height: 4096.0,
        ..viewport()
    };
    let outcome = paint_layout_with_vertical_overscan(
        &layout,
        &mut resolver,
        oversized,
        VerticalOverscanPlan::new(0, 0, 2, 0).unwrap(),
        &[],
        None,
        &[],
        &std::collections::HashMap::new(),
        &shadow_masks,
    );
    assert!(matches!(
        outcome,
        RetainedOverscanOutcome::Rejected {
            reason: RetainedPaintError::Capacity
        }
    ));
    let offset_viewport = Rect {
        x: 1.0,
        ..viewport()
    };
    assert!(matches!(
        paint_layout_with_vertical_overscan(
            &layout,
            &mut resolver,
            offset_viewport,
            VerticalOverscanPlan::new(0, 0, 1, 0).unwrap(),
            &[],
            None,
            &[],
            &std::collections::HashMap::new(),
            &shadow_masks,
        ),
        RetainedOverscanOutcome::Rejected {
            reason: RetainedPaintError::InvalidOverscanPlan
        }
    ));
    assert_eq!(super::retained_extract::retained_raster_count(), before);
}

#[test]
fn overscan_composition_keeps_visible_image_playback_metadata() {
    let document = TreeBuilder::parse(
        "<style>*{margin:0;padding:0}html{background:white}body{height:96px}\
         iframe{position:fixed;left:0;top:0;width:8px;height:8px;border:0}\
         a{position:absolute;left:16px;top:40px;width:8px;height:8px;background:red}\
         a:visited{background:blue}</style>\
         <iframe id=child></iframe><a id=link href=/visited></a>",
    )
    .document();
    let iframe = document.query_selector("#child").unwrap();
    let link = document.query_selector("#link").unwrap();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }
    let layout = layout_tree(&document, &mut resolver, viewport()).unwrap();
    let frame = |color| ImageFrame {
        image: Image::new(1, 1, vec![color, 0, 0, 255]).unwrap(),
        delay_ms: 100,
        disposal: GifDisposal::Keep,
    };
    let animation = Arc::new(ImageAnimation {
        frames: vec![frame(255), frame(128)],
        delays_ms: vec![100, 100],
        duration_ms: 200,
        plays: None,
    });
    let snapshots = std::collections::HashMap::from([(
        iframe.identity(),
        animation.playback_image(77, "test:animated", 0, 0),
    )]);
    let shadow_masks = super::border::FrameShadowMaskCache::new(false);
    let retained = expect_overscan(paint_layout_with_vertical_overscan(
        &layout,
        &mut resolver,
        viewport(),
        VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(),
        &[],
        None,
        &[link.identity()],
        &snapshots,
        &shadow_masks,
    ));
    for scroll_y in [0, 48, 64] {
        let canvas = retained.compose_integer((0, scroll_y)).unwrap();
        let playbacks = canvas.image_playbacks();
        assert_eq!(playbacks.len(), 1);
        assert_eq!(playbacks[0].document, 77);
    }
    let fractional = retained.compose_logical((0.0, 48.5), 1.0).unwrap();
    let playbacks = fractional.image_playbacks();
    assert_eq!(playbacks.len(), 1);
    assert_eq!(playbacks[0].document, 77);
    let current = retained.compose_integer((0, 32)).unwrap();
    assert_eq!(current.pixel(17, 9), Some(Color::rgb(0, 0, 255)));

    let mut unvisited_layout = layout.clone();
    apply_scroll_offsets(
        &mut unvisited_layout,
        &mut resolver,
        viewport(),
        (0.0, 32.0),
    );
    let unvisited = super::paint_layout_with_document_snapshots_cached(
        &unvisited_layout,
        &mut resolver,
        viewport(),
        Vec::new(),
        None,
        [],
        &snapshots,
        false,
        &shadow_masks,
    )
    .0;
    assert_eq!(unvisited.pixel(17, 9), Some(Color::rgb(255, 0, 0)));
}

#[test]
fn fractional_scaled_sampling_is_premultiplied_across_transparent_tile_edges() {
    let image = Image::new(
        1,
        3,
        vec![
            255, 0, 0, 0, // hidden red must not leak
            0, 0, 255, 255, // sampled blue
            0, 0, 255, 255, // full-coverage endpoint halo
        ],
    )
    .unwrap();
    let retained = RetainedOverscanFrame::from_test_parts(
        1,
        1,
        2.0,
        ((0, 0), (0, 1)),
        (0, 0),
        vec![(RetainedPaintSpace::Document, vec![((0, 0), image)])],
    );
    retained.validate_compositor(2.0).unwrap();
    assert_eq!(
        retained.compose_logical((0.0, 0.25), 2.0).unwrap().pixels(),
        [0, 0, 255, 128]
    );
    assert_eq!(
        retained.compose_logical((0.0, 0.5), 2.0).unwrap().pixels(),
        [0, 0, 255, 255]
    );
    assert_eq!(
        retained.validate_compositor(1.0),
        Err(RetainedPaintError::InvalidOverscanRasterScale)
    );
}

#[test]
fn fractional_composition_keeps_fixed_pixels_unmoved() {
    let document = Image::new(1, 3, vec![10, 0, 0, 255, 20, 0, 0, 255, 30, 0, 0, 255]).unwrap();
    let fixed = Image::new(1, 2, vec![0, 0, 255, 255, 0, 0, 0, 0]).unwrap();
    let retained = RetainedOverscanFrame::from_test_parts(
        1,
        2,
        1.0,
        ((0, 0), (0, 1)),
        (0, 0),
        vec![
            (RetainedPaintSpace::Document, vec![((0, 0), document)]),
            (RetainedPaintSpace::ViewportFixed, vec![((0, 0), fixed)]),
        ],
    );
    retained.validate_compositor(1.0).unwrap();
    assert_eq!(
        retained.compose_logical((0.0, 0.5), 1.0).unwrap().pixels(),
        [0, 0, 255, 255, 25, 0, 0, 255]
    );
}

#[test]
fn fractional_document_fixed_document_runs_keep_rgba8_order() {
    let half_red = Image::new(1, 2, vec![255, 0, 0, 255, 0, 0, 0, 0]).unwrap();
    let fixed_blue = Image::new(1, 1, vec![0, 0, 255, 128]).unwrap();
    let half_green = Image::new(1, 2, vec![0, 255, 0, 128, 0, 0, 0, 0]).unwrap();
    let retained = RetainedOverscanFrame::from_test_parts(
        1,
        1,
        1.0,
        ((0, 0), (0, 1)),
        (0, 0),
        vec![
            (RetainedPaintSpace::Document, vec![((0, 0), half_red)]),
            (
                RetainedPaintSpace::ViewportFixed,
                vec![((0, 0), fixed_blue)],
            ),
            (RetainedPaintSpace::Document, vec![((0, 0), half_green)]),
        ],
    );
    retained.validate_compositor(1.0).unwrap();
    assert_eq!(
        retained.compose_logical((0.0, 0.5), 1.0).unwrap().pixels(),
        [59, 78, 118, 208]
    );
}

#[test]
fn fractional_document_run_maps_image_playback_metadata() {
    let frame = |first, second| ImageFrame {
        image: Image::new(1, 2, vec![first, 0, 0, 255, second, 0, 0, 255]).unwrap(),
        delay_ms: 100,
        disposal: GifDisposal::Keep,
    };
    let animation = Arc::new(ImageAnimation {
        frames: vec![frame(255, 128), frame(64, 32)],
        delays_ms: vec![100, 100],
        duration_ms: 200,
        plays: None,
    });
    let retained = RetainedOverscanFrame::from_test_parts(
        1,
        1,
        1.0,
        ((0, 0), (0, 1)),
        (0, 0),
        vec![(
            RetainedPaintSpace::Document,
            vec![((0, 0), animation.playback_image(88, "test:document", 0, 0))],
        )],
    );
    retained.validate_compositor(1.0).unwrap();
    let canvas = retained.compose_logical((0.0, 0.5), 1.0).unwrap();
    let playbacks = canvas.image_playbacks();
    assert_eq!(playbacks.len(), 1);
    assert_eq!(playbacks[0].document, 88);
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
fn retained_clip_diagnostic_identifies_owned_geometry_without_changing_paint() {
    let html = "<style>*{margin:0;padding:0}#clip{width:24px;height:24px;overflow:hidden}\
        #pin{position:fixed;top:4px;left:20px;width:12px;height:12px;background:blue}</style>\
        <div id=clip><div id=pin></div></div>";
    let (layout, mut resolver) = build_raw_fixture(html, |_| {});
    let clip_id = layout.node.query_selector("#clip").unwrap().identity();
    let pin_id = layout.node.query_selector("#pin").unwrap().identity();
    let diagnostic =
        super::retained_extract::diagnose_cross_space_clip(&layout, &mut resolver).unwrap();
    let json = serde_json::to_value(diagnostic).unwrap();
    assert_eq!(json["node_id"], pin_id);
    assert_eq!(json["ancestors"].as_array().unwrap().len(), 1);
    assert_eq!(json["ancestors"][0]["node_id"], clip_id);
    assert_eq!(
        json["ancestors"][0]["raw_padding_rect"],
        serde_json::json!([0.0, 0.0, 24.0, 24.0])
    );
    assert_eq!(json["ancestors"][0]["overflow_x"], true);
    assert_eq!(json["ancestors"][0]["overflow_y"], true);
    assert_eq!(json["ancestors"][0]["clip_path"], false);
    assert_eq!(json["ancestors_truncated"], false);
    let outcome =
        paint_layout_with_retained_fallback(&layout, &mut resolver, viewport(), &[], None);
    let RetainedPaintOutcome::Fallback { canvas, reason, .. } = outcome else {
        panic!("clip must remain conservative")
    };
    assert_eq!(reason, RetainedPaintError::CrossSpaceClip);
    assert_eq!(
        canvas.pixels(),
        ordinary_fixture(html, (0.0, 0.0), |_| {}).pixels()
    );
}

#[test]
fn retained_clip_diagnostic_ignores_fixed_containing_blocks_and_fixed_own_clip() {
    for css in [
        "#clip{transform:translate(0px)}",
        "#clip{overflow:visible}#pin{overflow:hidden}",
    ] {
        let html = format!(
            "<style>*{{margin:0;padding:0}}#clip{{width:24px;height:24px;overflow:hidden}}\
            #pin{{position:fixed;top:4px;left:20px;width:12px;height:12px;background:blue}}\
            {css}</style><div id=clip><div id=pin></div></div>"
        );
        let (layout, mut resolver) = build_raw_fixture(&html, |_| {});
        assert!(
            super::retained_extract::diagnose_cross_space_clip(&layout, &mut resolver).is_none(),
            "{css}"
        );
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

/// Use a uniform document run so fractional interpolation adds no unrelated
/// moving-edge approximation to these exact RGBA clip invariance assertions.
fn invariant_clip_fixture(ancestor_css: &str, nested: bool) -> String {
    let nested_css = if nested {
        "#inner{width:32px;height:144px;overflow:hidden}"
    } else {
        ""
    };
    let pin = "<div id=pin></div>";
    let children = if nested {
        format!("<div id=inner>{pin}</div>")
    } else {
        pin.to_string()
    };
    format!(
        "<style>*{{margin:0;padding:0}}html{{background:white}}\
         body{{width:32px;height:160px;overflow:hidden;{ancestor_css}}}\
         {nested_css}#pin{{position:fixed;left:4px;top:4px;width:20px;height:20px;\
         background:blue}}</style>{children}"
    )
}

fn assert_invariant_clip_pixels(html: &str, first_y: i32, count: u8, offsets: &[f64]) {
    let frame = expect_overscan(overscan_fixture(
        html,
        VerticalOverscanPlan::new(0, first_y, count, count / 2).unwrap(),
        |_| {},
    ));
    let last_y = first_y + 32 * i32::from(count - 1);
    assert_eq!(frame.coverage(), ((0, first_y), (0, last_y)));
    assert_eq!(frame.runs().len(), 3);
    assert_eq!(frame.runs()[1].space(), RetainedPaintSpace::ViewportFixed);
    for &y in offsets {
        let expected = ordinary_fixture(html, (0.0, y as f32), |_| {});
        let actual = frame.compose_logical((0.0, y), 1.0).unwrap();
        assert_eq!(actual.pixels(), expected.pixels(), "scroll y={y}");
        assert_eq!(actual.pixel(5, 5), Some(Color::rgb(0, 0, 255)));
    }
}

#[test]
fn overscan_full_page_body_overflow_keeps_fixed_pixels_at_all_offsets() {
    let html = invariant_clip_fixture("", false);
    assert_invariant_clip_pixels(
        &html,
        0,
        5,
        &[0.0, 0.25, 15.5, 31.75, 32.0, 63.5, 96.25, 127.75, 128.0],
    );
    // The permission belongs only to the overscan preflight/sample path.
    let expected = ordinary_fixture(&html, (0.0, 32.0), |_| {});
    match retained_fixture(&html, (0.0, 32.0), |_| {}) {
        RetainedPaintOutcome::Fallback { canvas, reason, .. } => {
            assert_eq!(reason, RetainedPaintError::CrossSpaceClip);
            assert_same_canvas(&canvas, &expected);
        }
        _ => panic!("ordinary single-frame extraction must keep its conservative policy"),
    }
}

#[test]
fn overscan_nested_overflow_clips_cover_nonzero_and_fractional_offsets() {
    let html = invariant_clip_fixture("", true);
    assert_invariant_clip_pixels(
        &html,
        16,
        4,
        &[16.0, 16.25, 31.5, 48.0, 79.75, 80.0, 111.75, 112.0],
    );
}

fn assert_clip_overscan_fallback(html: &str, setup: impl Fn(&NodeHandle)) {
    let expected = ordinary_fixture(html, (0.0, 32.0), |document| setup(document));
    match overscan_fixture(
        html,
        VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(),
        |document| setup(document),
    ) {
        RetainedOverscanOutcome::Fallback {
            canvas,
            reason,
            discarded_tiles,
        } => {
            assert_eq!(reason, RetainedPaintError::CrossSpaceClip);
            assert_eq!(discarded_tiles, 0);
            assert_same_canvas(&canvas, &expected);
        }
        _ => panic!("unproven cross-space clip must fall back before sample collection"),
    }
}

#[test]
fn overscan_clip_proof_rejects_small_rounded_path_and_scrolled_ancestors() {
    for css in [
        "width:24px;height:24px",
        "height:95.75px", // last viewport extends to 96, even a subpixel gap is unsafe
        "border-radius:2px",
        "border-radius:50%",
        "clip-path:inset(0 2px)",
    ] {
        assert_clip_overscan_fallback(&invariant_clip_fixture(css, false), |_| {});
    }
    // A nonzero scroll ancestor can shift a later nested clip differently from
    // the window scroll. Do not reuse the plain document-translation proof.
    let scrolled = invariant_clip_fixture("", true).replace("height:144px", "height:192px");
    assert_clip_overscan_fallback(&scrolled, |document| {
        document
            .query_selector("body")
            .unwrap()
            .set_scroll_offset(0.0, 1.0);
    });
}

#[test]
fn overscan_equal_endpoint_fixed_pixels_do_not_prove_intermediate_clip_safety() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:128px}\
        #clip{position:absolute;left:0;top:32px;width:32px;height:8px;overflow:hidden}\
        #pin{position:fixed;left:4px;top:12px;width:20px;height:8px;background:blue}\
        </style><div id=clip><div id=pin></div></div>";
    let start = ordinary_fixture(html, (0.0, 0.0), |_| {});
    let end = ordinary_fixture(html, (0.0, 64.0), |_| {});
    assert_same_canvas(&start, &end);
    let middle = ordinary_fixture(html, (0.0, 16.0), |_| {});
    assert_ne!(start.pixels(), middle.pixels());
    assert_eq!(middle.pixel(5, 17), Some(Color::rgb(0, 0, 255)));
    assert_clip_overscan_fallback(html, |_| {});
}

#[test]
fn overscan_each_nested_clip_must_cover_the_full_interval() {
    let html = invariant_clip_fixture("", true).replace("height:144px", "height:80px");
    assert_clip_overscan_fallback(&html, |_| {});
}

#[test]
fn overscan_clip_proof_checks_unclipped_axes_independently() {
    for overflow in [
        "overflow-x:clip;overflow-y:visible",
        "overflow-x:visible;overflow-y:clip",
    ] {
        let html = invariant_clip_fixture(overflow, false);
        assert_invariant_clip_pixels(&html, 0, 3, &[0.0, 0.5, 31.75, 32.0, 63.5, 64.0]);
    }
}

#[test]
fn overscan_containment_and_transform_keep_fixed_children_in_document_space() {
    for declaration in [
        "contain:layout",
        "contain:paint",
        "transform:translate(0px)",
    ] {
        let html = invariant_clip_fixture(declaration, false);
        let frame = expect_overscan(overscan_fixture(
            &html,
            VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(),
            |_| {},
        ));
        assert!(
            frame
                .runs()
                .iter()
                .all(|run| run.space() == RetainedPaintSpace::Document)
        );
        for y in [0, 8, 16, 31, 32, 48, 64] {
            let expected = ordinary_fixture(&html, (0.0, y as f32), |_| {});
            assert_same_canvas(&frame.compose_integer((0, y)).unwrap(), &expected);
        }
    }
}

#[test]
fn overscan_proven_clip_does_not_override_a_later_unproven_scroll_ancestor() {
    let html = invariant_clip_fixture("", true).replace("overflow:hidden}", "overflow:visible}");
    assert_clip_overscan_fallback(&html, |document| {
        document
            .query_selector("#inner")
            .unwrap()
            .set_scroll_offset(0.0, 1.0);
    });
}

#[test]
fn overscan_invariant_clip_proves_the_constant_horizontal_scroll_too() {
    let html = invariant_clip_fixture("width:64px", false);
    let frame = expect_overscan(overscan_fixture(
        &html,
        VerticalOverscanPlan::new(8, 0, 3, 1).unwrap(),
        |_| {},
    ));
    for y in [0.0, 0.25, 16.5, 32.0, 63.75, 64.0] {
        let expected = ordinary_fixture(&html, (8.0, y as f32), |_| {});
        let actual = frame.compose_logical((8.0, y), 1.0).unwrap();
        assert_same_canvas(&actual, &expected);
    }
    assert_clip_overscan_fallback(&invariant_clip_fixture("width:31.75px", false), |_| {});
}

#[test]
fn overscan_pointwise_filters_match_ordinary_pixels_between_every_sample() {
    for filter in [
        "brightness(.65)",
        "contrast(1.25)",
        "grayscale(.6)",
        "hue-rotate(75deg)",
        "invert(.4)",
        "opacity(.6)",
        "saturate(1.5)",
        "sepia(.8)",
        "brightness(.8) contrast(1.2) opacity(.6) invert(.2)",
    ] {
        let html = format!(
            "<style>*{{margin:0;padding:0}}html{{background:white}}body{{height:160px}}\
             #target{{position:absolute;left:4px;top:24px;width:24px;height:24px;\
             background:rgba(180,60,120,.7);filter:{filter}}}\
             #pin{{position:fixed;left:0;top:0;width:3px;height:3px;background:blue}}\
             </style><div id=target></div><div id=pin></div>"
        );
        assert_ne!(
            ordinary_fixture(&html, (0.0, 0.0), |_| {}).pixels(),
            ordinary_fixture(
                &html.replace(&format!("filter:{filter}"), "filter:none"),
                (0.0, 0.0),
                |_| {}
            )
            .pixels(),
            "fixture must exercise an observable filter: {filter}"
        );
        let frame = expect_overscan(overscan_fixture(
            &html,
            VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(),
            |_| {},
        ));
        for y in 0..=64 {
            let actual = frame.compose_integer((0, y)).unwrap();
            let expected = ordinary_fixture(&html, (0.0, y as f32), |_| {});
            assert_eq!(actual.pixels(), expected.pixels(), "{filter}, y={y}");
        }
    }
}

#[test]
fn overscan_inline_pointwise_filters_keep_the_existing_fragment_painter() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #target{position:absolute;top:24px;left:0;width:32px}\
        span{filter:invert(.6) opacity(.7);background:rgb(180,60,120);font-size:12px}\
        </style><div id=target><span>MMMM</span></div>";
    let frame = expect_overscan(overscan_fixture(
        html,
        VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(),
        |_| {},
    ));
    for y in 0..=64 {
        assert_eq!(
            frame.compose_integer((0, y)).unwrap().pixels(),
            ordinary_fixture(html, (0.0, y as f32), |_| {}).pixels(),
            "inline filter y={y}"
        );
    }
}

#[test]
fn overscan_atomic_inline_pointwise_filter_keeps_the_existing_fragment_painter() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #hidden{visibility:hidden;height:1px}\
        #target{visibility:visible;position:absolute;top:24px;left:4px;width:24px;height:16px}\
        #atomic{display:inline-block;width:16px;height:12px;filter:invert(.6) opacity(.7);\
        background:rgb(180,60,120)}\
        </style><div id=hidden><div id=target><span id=atomic></span></div></div>";
    assert_ne!(
        ordinary_fixture(html, (0.0, 0.0), |_| {}).pixels(),
        ordinary_fixture(
            &html.replace("filter:invert(.6) opacity(.7)", "filter:none"),
            (0.0, 0.0),
            |_| {}
        )
        .pixels(),
        "the atomic-inline pointwise filter must be observable"
    );
    let frame = expect_overscan(overscan_fixture(
        html,
        VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(),
        |_| {},
    ));
    for y in 0..=64 {
        assert_eq!(
            frame.compose_integer((0, y)).unwrap().pixels(),
            ordinary_fixture(html, (0.0, y as f32), |_| {}).pixels(),
            "atomic inline filter y={y}"
        );
    }
}

#[test]
fn overscan_spatial_filter_chains_under_effect_ancestor_keep_exact_fallback() {
    for filter in [
        "brightness(.8) blur(2px)",
        "drop-shadow(2px 2px 0 red) invert(.5)",
        "opacity(.5) blur(1px)",
    ] {
        let html = format!(
            "<style>*{{margin:0;padding:0}}html{{background:white}}body{{height:160px}}\
             #target{{position:absolute;left:4px;top:24px;width:24px;height:24px;\
             background:red;filter:{filter}}}#wrap{{opacity:.8}}</style><div id=wrap><div id=target></div></div>"
        );
        match overscan_fixture(
            &html,
            VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(),
            |_| {},
        ) {
            RetainedOverscanOutcome::Fallback {
                canvas,
                reason: RetainedPaintError::ViewportEdgeFilter,
                discarded_tiles: 0,
            } => {
                assert_same_canvas(&canvas, &ordinary_fixture(&html, (0.0, 32.0), |_| {}));
            }
            _ => panic!("spatial chain must fall back: {filter}"),
        }
    }
}

fn assert_sticky_integer_pixels(html: &str, first: i32, samples: u8) -> RetainedOverscanFrame {
    let frame = expect_overscan(overscan_fixture(
        html,
        VerticalOverscanPlan::new(0, first, samples, 1).unwrap(),
        |_| {},
    ));
    let last = first + 32 * (samples as i32 - 1);
    for y in first..=last {
        let actual = frame.compose_integer((0, y)).unwrap();
        let expected = ordinary_fixture(html, (0.0, y as f32), |_| {});
        if actual.pixels() != expected.pixels() {
            eprintln!("scroll_y={y}, fixture={html}");
        }
        assert_same_canvas(&actual, &expected);
    }
    frame
}

fn assert_sticky_fractional_pixels(html: &str, frame: &RetainedOverscanFrame) {
    // These fixtures have no moving document-space edge in the viewport.
    // This checks sticky classification, separately from fractional tile interpolation.
    for y in [0.0, 0.25, 15.5, 31.75, 32.0, 63.75, 64.0] {
        assert_same_canvas(
            &frame.compose_logical((0.0, y), 1.0).unwrap(),
            &ordinary_fixture(html, (0.0, y as f32), |_| {}),
        );
    }
}

#[test]
fn overscan_origin_sticky_uses_ordered_fixed_run_with_positioned_descendants() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #pin{position:sticky;top:0;width:16px;height:16px;background:red}\
        #abs{position:absolute;left:8px;top:8px;width:8px;height:8px;background:blue}\
        #fixed{position:fixed;left:24px;top:4px;width:4px;height:4px;background:green}\
        #stripe{position:absolute;left:14px;top:0;width:4px;height:160px;background:purple}\
        </style><div id=pin><div id=abs></div><div id=fixed></div></div><div id=stripe></div>";
    let frame = assert_sticky_integer_pixels(html, 0, 3);
    assert_eq!(frame.runs().len(), 3);
    assert_eq!(frame.runs()[1].space(), RetainedPaintSpace::ViewportFixed);
    assert_sticky_fractional_pixels(html, &frame);
    assert!(matches!(
        retained_fixture(html, (0.0, 32.0), |_| {}),
        RetainedPaintOutcome::Fallback {
            reason: RetainedPaintError::StickyPosition,
            ..
        }
    ));
}

#[test]
fn overscan_document_sticky_auto_and_zero_scrollport_have_exact_pixels() {
    for (body, inset) in [("", ""), ("overflow:hidden;", "top:0;")] {
        let html = format!(
            "<style>*{{margin:0;padding:0}}html{{background:white}}body{{height:192px;{body}}}\
             #pin{{position:sticky;{inset}width:32px;height:160px;background:red}}\
             </style><div id=pin></div>"
        );
        let frame = assert_sticky_integer_pixels(&html, 0, 3);
        assert!(
            frame
                .runs()
                .iter()
                .all(|run| run.space() == RetainedPaintSpace::Document)
        );
        assert_sticky_fractional_pixels(&html, &frame);
    }
}

#[test]
fn overscan_sticky_containing_clamp_and_nonorigin_scrollport_are_document_space() {
    let clamped = "<style>*{margin:0;padding:0}html{background:white}body{height:192px}\
        #wrap{height:16px}#pin{position:sticky;top:0;width:16px;height:16px;background:red}\
        </style><div id=wrap><div id=pin></div></div>";
    let frame = assert_sticky_integer_pixels(clamped, 0, 3);
    assert!(
        frame
            .runs()
            .iter()
            .all(|run| run.space() == RetainedPaintSpace::Document)
    );
    let scrollport = "<style>*{margin:0;padding:0}html{background:white}\
        body{height:320px;overflow:hidden}#space{height:128px}\
        #pin{position:sticky;top:0;width:16px;height:16px;background:red}\
        </style><div id=space></div><div id=pin></div>";
    let frame = assert_sticky_integer_pixels(scrollport, 96, 3);
    assert!(
        frame
            .runs()
            .iter()
            .all(|run| run.space() == RetainedPaintSpace::Document)
    );
}

#[test]
fn overscan_document_sticky_parent_keeps_fixed_child_clip_constraints() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:192px;overflow:hidden}\
        #pin{position:sticky;width:24px;height:24px;overflow:hidden}\
        #fixed{position:fixed;left:20px;top:4px;width:12px;height:8px;background:blue}\
        </style><div id=pin><div id=fixed></div></div>";
    assert_clip_overscan_fallback(html, |_| {});
}

#[test]
fn overscan_equal_sticky_endpoint_pixels_do_not_prove_constant_phase() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:256px}\
        #space{height:32px}#wrap{height:48px}\
        #pin{position:sticky;top:0;width:16px;height:8px;background:red}\
        </style><div id=space></div><div id=wrap><div id=pin></div></div>";
    let start = ordinary_fixture(html, (0.0, 0.0), |_| {});
    let end = ordinary_fixture(html, (0.0, 128.0), |_| {});
    assert_same_canvas(&start, &end);
    assert_ne!(
        start.pixels(),
        ordinary_fixture(html, (0.0, 16.0), |_| {}).pixels()
    );
    match overscan_fixture(html, VerticalOverscanPlan::new(0, 0, 5, 2).unwrap(), |_| {}) {
        RetainedOverscanOutcome::Fallback {
            canvas,
            reason: RetainedPaintError::StickyPosition,
            ..
        } => {
            assert_same_canvas(&canvas, &ordinary_fixture(html, (0.0, 64.0), |_| {}));
        }
        _ => panic!("phase-changing sticky must fall back"),
    }
}

#[test]
fn overscan_sticky_uncertain_geometry_keeps_ordinary_fallback() {
    for (parent, sticky, height) in [
        ("", "top:8px", "160"),
        ("", "top:0", "79.75"),
        ("", "top:0", "80.5"),
        ("transform:translate(0px)", "top:0", "160"),
        ("position:sticky", "top:0", "160"),
    ] {
        let html = format!(
            "<style>*{{margin:0;padding:0}}html{{background:white}}body{{height:{height}px;{parent}}}\
             #pin{{position:sticky;{sticky};width:16px;height:16px;background:red}}\
             </style><div id=pin></div>"
        );
        match overscan_fixture(
            &html,
            VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(),
            |_| {},
        ) {
            RetainedOverscanOutcome::Fallback {
                canvas,
                reason: RetainedPaintError::StickyPosition,
                ..
            } => {
                assert_same_canvas(&canvas, &ordinary_fixture(&html, (0.0, 32.0), |_| {}));
            }
            _ => panic!("unproven geometry must fall back: {parent}; {sticky}; {height}"),
        }
    }
}

#[test]
fn overscan_document_sticky_parent_preserves_independent_fixed_child() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:192px;overflow:hidden}\
        #pin{position:sticky;width:16px;height:16px;background:red}\
        #fixed{position:fixed;left:20px;top:4px;width:8px;height:8px;background:blue}\
        </style><div id=pin><div id=fixed></div></div>";
    let frame = assert_sticky_integer_pixels(html, 32, 3);
    assert!(
        frame
            .runs()
            .iter()
            .any(|run| run.space() == RetainedPaintSpace::ViewportFixed)
    );
    for y in [32.0, 32.25, 63.75, 64.0, 95.75, 96.0] {
        assert_same_canvas(
            &frame.compose_logical((0.0, y), 1.0).unwrap(),
            &ordinary_fixture(html, (0.0, y as f32), |_| {}),
        );
    }
}

#[test]
fn overscan_sticky_nonzero_element_scroll_keeps_ordinary_fallback() {
    let html = "<style>*{margin:0;padding:0}html{background:white}\
        body{height:160px;overflow:hidden}#pin{position:sticky;top:0;height:192px;background:red}\
        </style><div id=pin></div>";
    let setup = |document: &NodeHandle| {
        document
            .query_selector("body")
            .unwrap()
            .set_scroll_offset(0.0, 1.0);
    };
    match overscan_fixture(html, VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(), setup) {
        RetainedOverscanOutcome::Fallback {
            canvas,
            reason: RetainedPaintError::StickyPosition,
            ..
        } => {
            assert_same_canvas(&canvas, &ordinary_fixture(html, (0.0, 32.0), setup));
        }
        _ => panic!("element scrolling must keep sticky fallback"),
    }
}

#[test]
fn overscan_source_filters_match_canonical_at_every_integer_offset() {
    for filter in [
        "blur(6px)",
        "drop-shadow(5px 3px 2px rgba(0,0,255,.7))",
        "drop-shadow(-5px -3px 2px rgba(0,0,255,.7))",
        "brightness(.7) blur(3px) opacity(.6)",
        "blur(2px) invert(.4) drop-shadow(-2px 3px 1px blue)",
    ] {
        let html = format!(
            "<style>*{{margin:0;padding:0}}html{{background:white}}body{{height:160px}}\
             #effect{{position:absolute;left:4px;top:24.5px;width:20px;height:24px;\
             background:rgba(255,0,0,.5);opacity:.8;filter:{filter}}}\
             #fixed{{position:fixed;left:14px;top:4px;width:8px;height:8px;background:blue}}\
             #later{{position:absolute;left:18px;top:0;width:4px;height:160px;background:purple}}\
             </style><div id=effect></div><div id=fixed></div><div id=later></div>"
        );
        let frame = assert_sticky_integer_pixels(&html, 0, 3);
        assert_eq!(
            frame.runs().len(),
            5,
            "ordered filter/fixed roots: {filter}"
        );
        assert!(
            frame
                .runs()
                .iter()
                .any(|run| matches!(run.identity(), RetainedPaintRunIdentity::FilterSource(_)))
        );
        assert!(frame.filter_source_storage_bytes() < 3 * 32 * 32 * 4);
        // The ordinary single-frame extractor retains its existing behavior.
        let single = expect_extracted(retained_fixture(&html, (0.0, 32.0), |_| {}));
        assert_same_canvas(
            &single.compose(),
            &ordinary_fixture(&html, (0.0, 32.0), |_| {}),
        );
    }
}

#[test]
fn overscan_source_filter_keeps_empty_sample_identity_and_sparse_pixels() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #effect{position:absolute;left:8px;top:44px;width:8px;height:8px;background:red;filter:blur(2px)}\
        </style><div id=effect></div>";
    let frame = expect_overscan(overscan_fixture(
        html,
        VerticalOverscanPlan::new(0, 0, 3, 0).unwrap(),
        |_| {},
    ));
    frame.validate_compositor(1.0).unwrap();
    assert_eq!(frame.filter_source_storage_bytes(), 8 * 8 * 4);
    assert_eq!(frame.runs().len(), 3);
    for y in 0..=64 {
        assert_same_canvas(
            &frame.compose_integer((0, y)).unwrap(),
            &ordinary_fixture(html, (0.0, y as f32), |_| {}),
        );
    }
}

#[test]
fn overscan_fully_outside_source_filter_keeps_owned_empty_identity_without_pixels() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:2000px}\
        #effect{position:absolute;top:1000px;width:8px;height:8px;background:red;filter:blur(10px)}\
        </style><div id=effect></div>";
    let frame = assert_sticky_integer_pixels(html, 0, 3);
    assert_eq!(frame.empty_filter_source_count(), 1);
    assert_eq!(frame.filter_source_storage_bytes(), 0);
    assert_eq!(frame.runs().len(), 1);
}

#[test]
fn overscan_source_filter_reproduces_canonical_group_buffer_at_viewport_edges() {
    let html = "<style>*{margin:0;padding:0}body{height:160px}\
        #effect{position:absolute;left:8px;top:1px;width:8px;height:2px;background:red;filter:blur(1px)}\
        </style><div id=effect></div>";
    let frame = assert_sticky_integer_pixels(html, 0, 3);
    let first = frame.compose_integer((0, 0)).unwrap();
    let moved = frame.compose_integer((0, 2)).unwrap();
    // One document pixel has different canonical alpha when it reaches the
    // viewport edge: this source recipe refilters instead of stitching it.
    assert_ne!(first.pixel(10, 2).unwrap().a, moved.pixel(10, 0).unwrap().a);
    assert_same_canvas(&moved, &ordinary_fixture(html, (0.0, 2.0), |_| {}));
}

#[test]
fn overscan_source_filter_multiple_groups_preserve_declared_and_paint_order() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        .effect{position:absolute;left:4px;top:24px;width:24px;height:24px}\
        #a{background:rgba(255,0,0,.5);filter:blur(4px) invert(.4)}\
        #b{left:8px;background:rgba(0,0,255,.5);filter:invert(.4) blur(4px)}\
        </style><div class=effect id=a></div><div class=effect id=b></div>";
    let frame = assert_sticky_integer_pixels(html, 0, 3);
    assert_eq!(frame.runs().len(), 5);
    assert_eq!(
        frame
            .runs()
            .iter()
            .filter(|run| matches!(run.identity(), RetainedPaintRunIdentity::FilterSource(_)))
            .count(),
        2
    );
}

#[test]
fn overscan_source_filter_invariant_body_clip_keeps_exact_current_painter() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:192px;overflow:hidden}\
        #effect{position:absolute;left:4px;top:24px;width:24px;height:24px;background:red;filter:blur(10px)}\
        </style><div id=effect></div>";
    assert_sticky_integer_pixels(html, 0, 3)
        .validate_compositor(1.0)
        .unwrap();
}

fn partial_rounded_overflow_filter_fixture(radius: &str) -> String {
    format!(
        "<style>*{{margin:0;padding:0}}body{{height:160px}}\
         #clip{{position:absolute;left:3.5px;top:20.5px;width:24.25px;height:18.5px;\
         overflow:hidden;border-radius:{radius}}}\
         #effect{{width:32px;height:32px;background:rgba(255,0,0,.75);filter:blur(10px)}}\
         </style><div id=clip><div id=effect></div></div>"
    )
}

#[test]
fn ordinary_source_filter_overflow_clip_ignores_decorative_border_radius() {
    let rounded = partial_rounded_overflow_filter_fixture("4px");
    let square = partial_rounded_overflow_filter_fixture("0");
    let rounded = ordinary_fixture(&rounded, (0.0, 0.0), |_| {});
    let square = ordinary_fixture(&square, (0.0, 0.0), |_| {});
    assert_same_canvas(&rounded, &square);

    let corner = rounded.pixel(3, 20).unwrap();
    assert!(
        corner.a > 0,
        "the canonical rectangular corner must remain painted"
    );
    assert_eq!(rounded.pixel(2, 19).unwrap().a, 0);
}

#[test]
fn overscan_source_filter_partial_rounded_overflow_matches_every_integer_scroll() {
    let html = partial_rounded_overflow_filter_fixture("4px");
    let frame = assert_sticky_integer_pixels(&html, 0, 3);
    frame.validate_compositor(1.0).unwrap();
    assert_eq!(
        frame
            .runs()
            .iter()
            .filter(|run| matches!(run.identity(), RetainedPaintRunIdentity::FilterSource(_)))
            .count(),
        1
    );
    let current = frame.compose_integer((0, 0)).unwrap();
    assert!(current.pixel(3, 20).unwrap().a > 0);
    assert_eq!(current.pixel(2, 19).unwrap().a, 0);
}

#[test]
fn overscan_source_filter_fully_clipped_current_sample_keeps_run_identity() {
    let html = partial_rounded_overflow_filter_fixture("4px");
    let frame = expect_overscan(overscan_fixture(
        &html,
        VerticalOverscanPlan::new(0, 0, 3, 2).unwrap(),
        |_| {},
    ));
    assert_eq!(frame.current_scroll(), (0, 64));
    assert_eq!(frame.runs().len(), 3);
    assert_eq!(
        frame.runs()[0].identity(),
        RetainedPaintRunIdentity::Document(0)
    );
    assert!(matches!(
        frame.runs()[1].identity(),
        RetainedPaintRunIdentity::FilterSource(_)
    ));
    assert_eq!(frame.runs()[1].filter_source_sample_count(), Some(3));
    assert_eq!(
        frame.runs()[2].identity(),
        RetainedPaintRunIdentity::Document(1)
    );
    for y in 0..=64 {
        assert_same_canvas(
            &frame.compose_integer((0, y)).unwrap(),
            &ordinary_fixture(&html, (0.0, y as f32), |_| {}),
        );
    }
}

#[test]
fn overscan_clipped_normal_flow_targets_keep_exact_separator_order() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #clip{margin:20px 0 0 2px;width:28px;height:20px;overflow:hidden}\
        #before,#between,#after{height:2px;background:rgba(0,255,0,.55)}\
        .effect{width:26px;height:6px;filter:blur(2px)}\
        #a{overflow:hidden;background:red}\
        .wide{width:32px;height:8px;background:blue}\
        #b{background:rgba(255,0,255,.5)}\
        </style><div id=clip><div id=before></div><div class=effect id=a><div class=wide></div></div>\
        <div id=between></div><div class=effect id=b></div><div id=after></div></div>";
    let frame = expect_overscan(overscan_fixture(
        html,
        VerticalOverscanPlan::new(0, 0, 3, 2).unwrap(),
        |_| {},
    ));
    let identities = frame
        .runs()
        .iter()
        .map(|run| run.identity())
        .collect::<Vec<_>>();
    assert_eq!(identities.len(), 5);
    assert!(matches!(
        identities[0],
        RetainedPaintRunIdentity::Document(0)
    ));
    assert!(matches!(
        identities[1],
        RetainedPaintRunIdentity::FilterSource(_)
    ));
    assert!(matches!(
        identities[2],
        RetainedPaintRunIdentity::Document(1)
    ));
    assert!(matches!(
        identities[3],
        RetainedPaintRunIdentity::FilterSource(_)
    ));
    assert!(matches!(
        identities[4],
        RetainedPaintRunIdentity::Document(2)
    ));
    assert_ne!(identities[1], identities[3]);
    assert_eq!(frame.runs()[1].filter_source_sample_count(), Some(3));
    assert_eq!(frame.runs()[3].filter_source_sample_count(), Some(3));
    for y in 0..=64 {
        assert_same_canvas(
            &frame.compose_integer((0, y)).unwrap(),
            &ordinary_fixture(html, (0.0, y as f32), |_| {}),
        );
    }
}

#[test]
fn overscan_nested_clip_hooks_emit_each_target_once_across_cull_levels() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #outer{margin-top:20px;width:30px;height:40px;overflow:hidden}\
        #inner{width:26px;height:8px;overflow:hidden}\
        #effect{width:24px;height:16px;background:rgba(255,0,0,.7);filter:blur(3px)}\
        </style><div id=outer><div id=inner><div id=effect></div></div></div>";
    let frame = expect_overscan(overscan_fixture(
        html,
        VerticalOverscanPlan::new(0, 0, 3, 2).unwrap(),
        |_| {},
    ));
    assert_eq!(frame.runs().len(), 3);
    assert_eq!(
        frame
            .runs()
            .iter()
            .filter(|run| matches!(run.identity(), RetainedPaintRunIdentity::FilterSource(_)))
            .count(),
        1
    );
    assert_eq!(frame.runs()[1].filter_source_sample_count(), Some(3));
    for y in 0..=64 {
        assert_same_canvas(
            &frame.compose_integer((0, y)).unwrap(),
            &ordinary_fixture(html, (0.0, y as f32), |_| {}),
        );
    }
}

#[test]
fn overscan_clipped_filter_phase_order_uncertainty_falls_back_before_capture() {
    let owner_lines = "<style>*{margin:0;padding:0}body{height:160px}\
        #clip{width:24px;height:24px;overflow:hidden}\
        #effect{display:block;width:24px;height:24px;background:red;filter:blur(3px)}\
        </style><div id=clip><span>line</span><div id=effect></div></div>";
    let explicit_z = "<style>*{margin:0;padding:0}body{height:160px}\
        #clip{display:flex;width:24px;height:24px;overflow:hidden}\
        #branch{z-index:1}#effect{width:24px;height:24px;background:red;filter:blur(3px)}\
        </style><div id=clip><div id=branch><div id=effect></div></div></div>";
    for html in [owner_lines, explicit_z] {
        match overscan_fixture(html, VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(), |_| {}) {
            RetainedOverscanOutcome::Fallback {
                canvas,
                reason: RetainedPaintError::ViewportEdgeFilter,
                discarded_tiles: 0,
            } => assert_same_canvas(&canvas, &ordinary_fixture(html, (0.0, 32.0), |_| {})),
            _ => panic!("uncertain clipped target paint order must fall back"),
        }
    }
}

#[test]
fn overscan_skipped_content_visibility_does_not_publish_hidden_filter_targets() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #skip{content-visibility:hidden;width:24px;height:8px;overflow:hidden;background:blue}\
        #hidden{width:32px;height:16px;background:red;filter:blur(3px)}\
        #clip{margin-top:12px;width:24px;height:12px;overflow:hidden}\
        #visible{width:28px;height:16px;background:green;filter:blur(3px)}\
        </style><div id=skip><div id=hidden></div></div>\
        <div id=clip><div id=visible></div></div>";
    let document = TreeBuilder::parse(html).document();
    let skipped = document.query_selector("#skip").unwrap();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }
    let (layout, report) = crate::layout::layout_tree_with_content_visibility(
        &document,
        &mut resolver,
        viewport(),
        crate::layout::ContentVisibilityLayoutInput {
            visible_rect: viewport(),
            forced_nodes: std::collections::HashSet::from([skipped.identity()]),
            ..crate::layout::ContentVisibilityLayoutInput::default()
        },
    );
    assert!(report.skipped_nodes.contains(&skipped.identity()));
    let layout = layout.unwrap();
    let mut pending = vec![&layout];
    let skipped_layout = loop {
        let current = pending.pop().expect("skipped node must remain in layout");
        if current.node == skipped {
            break current;
        }
        pending.extend(current.children.iter());
    };
    assert!(skipped_layout.content_visibility_contents_skipped);
    assert!(!skipped_layout.children.is_empty());
    let shadow_masks = super::border::FrameShadowMaskCache::new(false);
    let frame = expect_overscan(paint_layout_with_vertical_overscan(
        &layout,
        &mut resolver,
        viewport(),
        VerticalOverscanPlan::new(0, 0, 3, 2).unwrap(),
        &[],
        None,
        &[],
        &std::collections::HashMap::new(),
        &shadow_masks,
    ));
    assert_eq!(
        frame
            .runs()
            .iter()
            .filter(|run| matches!(run.identity(), RetainedPaintRunIdentity::FilterSource(_)))
            .count(),
        1
    );
    for y in 0..=64 {
        let mut adjusted = layout.clone();
        apply_scroll_offsets(&mut adjusted, &mut resolver, viewport(), (0.0, y as f32));
        let expected =
            paint_layout_with_web_fonts(&adjusted, &mut resolver, viewport(), Vec::new(), None);
        assert_same_canvas(&frame.compose_integer((0, y)).unwrap(), &expected);
    }
}

fn positioned_phase_filter_fixture() -> &'static str {
    "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
     #clip{position:relative;margin:18px 0 0 2px;width:27px;height:19px;overflow:hidden;\
     background:rgba(0,0,255,.35)}\
     #before,#after{height:4px;background:rgba(0,255,0,.45)}\
     #effect{position:absolute;left:8px;top:9px;width:24px;height:18px;\
     background:rgba(255,0,0,.7);filter:blur(4px)}\
     </style><div id=clip><div id=before></div><div id=effect></div><div id=after></div></div>"
}

#[test]
fn overscan_rejects_unplanned_filter_hoisted_through_hidden_flow_ancestor() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #hidden{visibility:hidden;height:1px}\
        #effect{visibility:visible;position:absolute;left:8px;top:44px;width:8px;height:8px;\
        background:red;filter:blur(4px)}\
        </style><div id=hidden><div id=effect></div></div>";
    let ordinary = ordinary_fixture(html, (0.0, 32.0), |_| {});
    let center = ordinary.pixel(12, 16).unwrap();
    assert!(center.r > center.g && center.r > center.b);
    let without_source = ordinary_fixture(
        &html.replace("<div id=effect></div>", ""),
        (0.0, 32.0),
        |_| {},
    );
    assert_ne!(ordinary.pixels(), without_source.pixels());
    match overscan_fixture(html, VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(), |_| {}) {
        RetainedOverscanOutcome::Fallback {
            canvas,
            reason: RetainedPaintError::ViewportEdgeFilter,
            discarded_tiles: 0,
        } => assert_same_canvas(&canvas, &ordinary),
        _ => panic!("a phase-hoisted filter omitted by the DOM walk must fall back before capture"),
    }
}

#[test]
fn overscan_positioned_filter_uses_its_actual_clipped_phase_owner() {
    let html = positioned_phase_filter_fixture();
    let frame = expect_overscan(overscan_fixture(
        html,
        VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(),
        |_| {},
    ));
    assert_eq!(frame.runs().len(), 3);
    assert_eq!(
        frame.runs()[0].identity(),
        RetainedPaintRunIdentity::Document(0)
    );
    assert!(matches!(
        frame.runs()[1].identity(),
        RetainedPaintRunIdentity::FilterSource(_)
    ));
    assert_eq!(frame.runs()[1].filter_source_sample_count(), Some(3));
    assert_eq!(
        frame.runs()[2].identity(),
        RetainedPaintRunIdentity::Document(1)
    );
    for y in 0..=64 {
        assert_same_canvas(
            &frame.compose_integer((0, y)).unwrap(),
            &ordinary_fixture(html, (0.0, y as f32), |_| {}),
        );
    }
}

#[test]
fn overscan_positioned_filter_bypasses_static_dom_clip_below_phase_owner() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #owner{position:relative;margin:17px 0 0 2px;width:26px;height:18px;overflow:hidden}\
        #middle{margin-left:12px;width:5px;height:7px;overflow:hidden}\
        #effect{position:absolute;left:0;top:3px;width:22px;height:10px;\
        background:red;filter:blur(2px)}\
        </style><div id=owner><div id=middle><div id=effect></div></div></div>";
    let ordinary = ordinary_fixture(html, (0.0, 0.0), |_| {});
    let outside_middle = ordinary.pixel(4, 24).unwrap();
    assert!(
        outside_middle.r > outside_middle.g && outside_middle.r > outside_middle.b,
        "the hoisted source must paint outside its static DOM parent's clip"
    );
    assert_eq!(ordinary.pixel(1, 24), Some(Color::rgb(255, 255, 255)));

    let frame = expect_overscan(overscan_fixture(
        html,
        VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(),
        |_| {},
    ));
    assert_eq!(frame.runs().len(), 3);
    assert!(matches!(
        frame.runs()[1].identity(),
        RetainedPaintRunIdentity::FilterSource(_)
    ));
    assert_eq!(frame.runs()[1].filter_source_sample_count(), Some(3));
    for y in 0..=64 {
        assert_eq!(
            frame.compose_integer((0, y)).unwrap().pixels(),
            ordinary_fixture(html, (0.0, y as f32), |_| {}).pixels(),
            "y={y}"
        );
    }
}

#[test]
fn overscan_rejects_fixed_filter_hoisted_through_hidden_flow_ancestor() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #hidden{visibility:hidden;height:1px}\
        #effect{visibility:visible;position:fixed;left:8px;top:12px;width:8px;height:8px;\
        background:red;filter:blur(4px)}\
        </style><div id=hidden><div id=effect></div></div>";
    let expected = ordinary_fixture(html, (0.0, 32.0), |_| {});
    let center = expected.pixel(12, 16).unwrap();
    assert!(center.r > center.g && center.r > center.b);
    match overscan_fixture(html, VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(), |_| {}) {
        RetainedOverscanOutcome::Fallback {
            canvas,
            reason: RetainedPaintError::ViewportEdgeFilter,
            discarded_tiles: 0,
        } => assert_same_canvas(&canvas, &expected),
        _ => panic!("a phase-hoisted fixed source must fall back before capture"),
    }
}

#[test]
fn overscan_rejects_inline_filter_hoisted_through_hidden_flow_ancestor() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #hidden{visibility:hidden;height:1px}\
        #container{visibility:visible;position:absolute;left:4px;top:44px;width:24px;height:16px}\
        span{font-size:12px;color:red;background:red;filter:blur(3px)}\
        </style><div id=hidden><div id=container><span>MMMM</span></div></div>";
    let expected = ordinary_fixture(html, (0.0, 32.0), |_| {});
    assert!(
        expected
            .pixels()
            .chunks_exact(4)
            .any(|pixel| pixel[0] > pixel[1] && pixel[0] > pixel[2]),
        "the hoisted inline filter must contribute red pixels"
    );
    let without_source =
        ordinary_fixture(&html.replace("<span>MMMM</span>", ""), (0.0, 32.0), |_| {});
    assert_ne!(expected.pixels(), without_source.pixels());
    match overscan_fixture(html, VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(), |_| {}) {
        RetainedOverscanOutcome::Fallback {
            canvas,
            reason: RetainedPaintError::ViewportEdgeFilter,
            discarded_tiles: 0,
        } => assert_same_canvas(&canvas, &expected),
        _ => panic!("a phase-hoisted inline source must fall back before capture"),
    }
}

#[test]
fn overscan_rejects_atomic_inline_filter_hoisted_through_hidden_flow_ancestor() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #hidden{visibility:hidden;height:1px}\
        #container{visibility:visible;position:absolute;left:4px;top:44px;width:24px;height:16px}\
        #atomic{display:inline-block;width:16px;height:12px;background:red;filter:blur(3px)}\
        </style><div id=hidden><div id=container><span id=atomic></span></div></div>";
    let expected = ordinary_fixture(html, (0.0, 32.0), |_| {});
    assert!(
        expected
            .pixels()
            .chunks_exact(4)
            .any(|pixel| pixel[0] > pixel[1] && pixel[0] > pixel[2]),
        "the phase-hoisted atomic inline filter must contribute red pixels"
    );
    let without_source = ordinary_fixture(
        &html.replace("<span id=atomic></span>", ""),
        (0.0, 32.0),
        |_| {},
    );
    assert_ne!(expected.pixels(), without_source.pixels());
    match overscan_fixture(html, VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(), |_| {}) {
        RetainedOverscanOutcome::Fallback {
            canvas,
            reason: RetainedPaintError::ViewportEdgeFilter,
            discarded_tiles: 0,
        } => assert_same_canvas(&canvas, &expected),
        _ => panic!("a phase-hoisted atomic inline source must fall back before capture"),
    }
}

#[test]
fn overscan_rejects_nested_filter_source_hoisted_through_hidden_flow_ancestor() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #outer{position:relative;margin-top:40px;width:24px;height:16px;filter:blur(4px)}\
        #hidden{visibility:hidden}\
        #inner{visibility:visible;position:absolute;left:4px;top:2px;width:16px;height:12px;\
        background:red;filter:blur(2px)}\
        </style><div id=outer><div id=hidden><div id=inner></div></div></div>";
    let expected = ordinary_fixture(html, (0.0, 32.0), |_| {});
    let center = expected.pixel(12, 16).unwrap();
    assert!(center.r > center.g && center.r > center.b);
    let control = ordinary_fixture(
        &html.replace("<div id=inner></div>", ""),
        (0.0, 32.0),
        |_| {},
    );
    assert_ne!(expected.pixels(), control.pixels());
    match overscan_fixture(html, VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(), |_| {}) {
        RetainedOverscanOutcome::Fallback {
            canvas,
            reason: RetainedPaintError::ViewportEdgeFilter,
            discarded_tiles: 0,
        } => assert_same_canvas(&canvas, &expected),
        _ => panic!("a nested source hoisted through hidden flow must fall back before capture"),
    }
}

#[test]
fn overscan_rejects_pruned_unsupported_descendants_of_an_active_filter_source() {
    for (declaration, expected_reason) in [
        ("clip-path:inset(0)", RetainedPaintError::ViewportEdgeFilter),
        (
            "mask-image:linear-gradient(black,black)",
            RetainedPaintError::ViewportEdgeFilter,
        ),
        ("contain:paint", RetainedPaintError::ViewportEdgeFilter),
        (
            "backdrop-filter:blur(2px)",
            RetainedPaintError::BackdropFilter,
        ),
    ] {
        let html = format!(
            "<style>*{{margin:0;padding:0}}html{{background:white}}body{{height:160px}}\
             #outer{{position:relative;margin-top:40px;width:24px;height:16px;filter:blur(4px)}}\
             #hidden{{visibility:hidden}}\
             #inner{{visibility:visible;position:absolute;left:4px;top:2px;width:16px;height:12px;\
             background:red;{declaration}}}\
             </style><div id=outer><div id=hidden><div id=inner></div></div></div>"
        );
        let expected = ordinary_fixture(&html, (0.0, 32.0), |_| {});
        let center = expected.pixel(12, 16).unwrap();
        assert!(
            center.r > center.g && center.r > center.b,
            "the active source descendant must contribute red: {declaration}"
        );
        let control = ordinary_fixture(
            &html.replace("<div id=inner></div>", ""),
            (0.0, 32.0),
            |_| {},
        );
        assert_ne!(
            expected.pixels(),
            control.pixels(),
            "the source descendant must be observable: {declaration}"
        );
        match overscan_fixture(
            &html,
            VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(),
            |_| {},
        ) {
            RetainedOverscanOutcome::Fallback {
                canvas,
                reason,
                discarded_tiles: 0,
            } => {
                assert_eq!(reason, expected_reason, "{declaration}");
                assert_same_canvas(&canvas, &expected);
            }
            _ => panic!("a pruned active-source descendant must fall back: {declaration}"),
        }
    }
}

#[test]
fn overscan_rejects_inline_spatial_filter_owned_by_an_active_filter_source() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #outer{margin-top:40px;width:24px;height:16px;filter:blur(4px);font-size:12px}\
        span{color:red;background:red;filter:blur(2px)}\
        </style><div id=outer><span>MMMM</span></div>";
    let (layout, _) = build_raw_fixture(html, |_| {});
    let outer = layout.node.query_selector("#outer").unwrap();
    let mut pending = vec![&layout];
    let outer_layout = loop {
        let current = pending.pop().expect("outer layout box");
        if current.node == outer {
            break current;
        }
        pending.extend(current.children.iter());
    };
    assert!(outer_layout.lines.iter().any(|line| {
        line.fragments.iter().any(|fragment| {
            let InlineFragmentContent::InlineBox(style) = &fragment.content else {
                return false;
            };
            super::element_filters(style)
                .iter()
                .any(|filter| {
                    matches!(filter, crate::css::FilterFunction::Blur(radius) if *radius == 2.0)
                })
        })
    }));
    let expected = ordinary_fixture(html, (0.0, 32.0), |_| {});
    assert!(
        expected
            .pixels()
            .chunks_exact(4)
            .any(|pixel| pixel[0] > pixel[1] && pixel[0] > pixel[2]),
        "the inline fragment content must contribute red pixels"
    );
    let control = ordinary_fixture(&html.replace("<span>MMMM</span>", ""), (0.0, 32.0), |_| {});
    assert_ne!(expected.pixels(), control.pixels());
    match overscan_fixture(html, VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(), |_| {}) {
        RetainedOverscanOutcome::Fallback {
            canvas,
            reason: RetainedPaintError::ViewportEdgeFilter,
            discarded_tiles: 0,
        } => assert_same_canvas(&canvas, &expected),
        _ => panic!("an active source's own inline spatial filter must fall back before capture"),
    }
}

#[test]
fn overscan_keeps_pruned_pointwise_descendant_inside_an_active_filter_source() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #outer{position:relative;margin-top:24px;width:24px;height:16px;filter:blur(4px)}\
        #hidden{visibility:hidden}\
        #inner{visibility:visible;position:absolute;left:4px;top:2px;width:16px;height:12px;\
        background:rgb(180,60,120);filter:invert(.6) opacity(.7)}\
        </style><div id=outer><div id=hidden><div id=inner></div></div></div>";
    assert_ne!(
        ordinary_fixture(html, (0.0, 0.0), |_| {}).pixels(),
        ordinary_fixture(
            &html.replace("filter:invert(.6) opacity(.7)", "filter:none"),
            (0.0, 0.0),
            |_| {},
        )
        .pixels(),
        "the hoisted pointwise filter must be observable inside the outer source"
    );
    let frame = expect_overscan(overscan_fixture(
        html,
        VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(),
        |_| {},
    ));
    let source_runs = frame
        .runs()
        .iter()
        .filter(|run| matches!(run.identity(), RetainedPaintRunIdentity::FilterSource(_)))
        .collect::<Vec<_>>();
    assert_eq!(source_runs.len(), 1);
    assert_eq!(source_runs[0].filter_source_sample_count(), Some(3));
    for y in 0..=64 {
        assert_eq!(
            frame.compose_integer((0, y)).unwrap().pixels(),
            ordinary_fixture(html, (0.0, y as f32), |_| {}).pixels(),
            "y={y}"
        );
    }
}

fn build_forced_skipped_source_fixture(html: &str) -> (LayoutBox, StyleResolver) {
    let document = TreeBuilder::parse(html).document();
    let skipped = document.query_selector("#skip").unwrap();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }
    let (layout, report) = crate::layout::layout_tree_with_content_visibility(
        &document,
        &mut resolver,
        viewport(),
        crate::layout::ContentVisibilityLayoutInput {
            visible_rect: viewport(),
            forced_nodes: std::collections::HashSet::from([skipped.identity()]),
            ..crate::layout::ContentVisibilityLayoutInput::default()
        },
    );
    assert!(report.skipped_nodes.contains(&skipped.identity()));
    let layout = layout.unwrap();
    let mut pending = vec![&layout];
    let skipped_layout = loop {
        let current = pending.pop().expect("skipped node must remain in layout");
        if current.node == skipped {
            break current;
        }
        pending.extend(current.children.iter());
    };
    assert!(skipped_layout.content_visibility_contents_skipped);
    (layout, resolver)
}

fn paint_forced_skipped_source_fixture(html: &str, scroll_y: f32) -> Canvas {
    let (mut layout, mut resolver) = build_forced_skipped_source_fixture(html);
    apply_scroll_offsets(&mut layout, &mut resolver, viewport(), (0.0, scroll_y));
    paint_layout_with_web_fonts(&layout, &mut resolver, viewport(), Vec::new(), None)
}

#[test]
fn overscan_rejects_nested_filter_source_hoisted_through_skipped_contents() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #outer{position:relative;margin-top:40px;width:24px;height:16px;filter:blur(4px)}\
        #skip{content-visibility:hidden}\
        #inner{position:absolute;left:4px;top:2px;width:16px;height:12px;\
        background:red;filter:blur(2px)}\
        </style><div id=outer><div id=skip><div id=inner></div></div></div>";
    let expected = paint_forced_skipped_source_fixture(html, 32.0);
    let center = expected.pixel(12, 16).unwrap();
    assert!(center.r > center.g && center.r > center.b);
    let control =
        paint_forced_skipped_source_fixture(&html.replace("<div id=inner></div>", ""), 32.0);
    assert_ne!(expected.pixels(), control.pixels());

    let (layout, mut resolver) = build_forced_skipped_source_fixture(html);
    let shadow_masks = super::border::FrameShadowMaskCache::new(false);
    match paint_layout_with_vertical_overscan(
        &layout,
        &mut resolver,
        viewport(),
        VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(),
        &[],
        None,
        &[],
        &std::collections::HashMap::new(),
        &shadow_masks,
    ) {
        RetainedOverscanOutcome::Fallback {
            canvas,
            reason: RetainedPaintError::ViewportEdgeFilter,
            discarded_tiles: 0,
        } => assert_same_canvas(&canvas, &expected),
        _ => panic!("a nested source hoisted through skipped contents must fall back"),
    }
}

#[test]
fn overscan_rejects_unplanned_fixed_root_hoisted_through_hidden_flow_ancestor() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #hidden{visibility:hidden;height:1px}\
        #fixed{visibility:visible;position:fixed;left:8px;top:12px;width:8px;height:8px;\
        background:red}\
        </style><div id=hidden><div id=fixed></div></div>";
    let expected = ordinary_fixture(html, (0.0, 32.0), |_| {});
    assert_eq!(expected.pixel(12, 16), Some(Color::rgb(255, 0, 0)));
    let control = ordinary_fixture(
        &html.replace("<div id=fixed></div>", ""),
        (0.0, 32.0),
        |_| {},
    );
    assert_ne!(expected.pixels(), control.pixels());
    match overscan_fixture(html, VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(), |_| {}) {
        RetainedOverscanOutcome::Fallback {
            canvas,
            reason: RetainedPaintError::TraversalMismatch,
            discarded_tiles: 0,
        } => assert_same_canvas(&canvas, &expected),
        _ => panic!("a phase-hoisted fixed root missing from raw classification must fall back"),
    }
}

#[test]
fn overscan_rejects_fixed_background_hoisted_through_hidden_flow_ancestor() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #hidden{visibility:hidden;height:1px}\
        #effect{visibility:visible;position:absolute;left:4px;top:44px;width:24px;height:12px;\
        background-image:linear-gradient(red,red);background-attachment:fixed}\
        </style><div id=hidden><div id=effect></div></div>";
    let expected = ordinary_fixture(html, (0.0, 32.0), |_| {});
    let center = expected.pixel(12, 16).unwrap();
    assert!(center.r > center.g && center.r > center.b);
    let control = ordinary_fixture(
        &html.replace("<div id=effect></div>", ""),
        (0.0, 32.0),
        |_| {},
    );
    assert_ne!(expected.pixels(), control.pixels());
    match overscan_fixture(html, VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(), |_| {}) {
        RetainedOverscanOutcome::Fallback {
            canvas,
            reason: RetainedPaintError::FixedBackground,
            discarded_tiles: 0,
        } => assert_same_canvas(&canvas, &expected),
        _ => panic!("a phase-hoisted fixed background must retain its classifier fallback"),
    }
}

#[test]
fn overscan_rejects_backdrop_filter_hoisted_through_hidden_flow_ancestor() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #under{position:absolute;left:0;top:40px;width:32px;height:20px;\
        background:linear-gradient(90deg,red 50%,blue 50%)}\
        #hidden{visibility:hidden;height:1px}\
        #effect{visibility:visible;position:absolute;left:8px;top:44px;width:16px;height:12px;\
        background:rgba(255,255,255,.15);backdrop-filter:blur(3px)}\
        </style><div id=under></div><div id=hidden><div id=effect></div></div>";
    let expected = ordinary_fixture(html, (0.0, 32.0), |_| {});
    let control = ordinary_fixture(
        &html.replace("backdrop-filter:blur(3px)", "backdrop-filter:none"),
        (0.0, 32.0),
        |_| {},
    );
    assert_ne!(expected.pixels(), control.pixels());
    match overscan_fixture(html, VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(), |_| {}) {
        RetainedOverscanOutcome::Fallback {
            canvas,
            reason: RetainedPaintError::BackdropFilter,
            discarded_tiles: 0,
        } => assert_same_canvas(&canvas, &expected),
        _ => panic!("a phase-hoisted backdrop filter must retain its classifier fallback"),
    }
}

#[test]
fn overscan_rejects_sticky_missing_from_the_raw_sticky_plan() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #hidden{visibility:hidden;height:160px;padding-top:1px}\
        #sticky{visibility:visible;position:sticky;top:0;margin-top:12px;width:12px;height:10px;\
        background:red}\
        </style><div id=hidden><div id=sticky></div></div>";
    let expected = ordinary_fixture(html, (0.0, 32.0), |_| {});
    assert!(
        expected
            .pixels()
            .chunks_exact(4)
            .any(|pixel| pixel[0] > pixel[1] && pixel[0] > pixel[2])
    );
    let control = ordinary_fixture(
        &html.replace("<div id=sticky></div>", ""),
        (0.0, 32.0),
        |_| {},
    );
    assert_ne!(expected.pixels(), control.pixels());
    match overscan_fixture(html, VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(), |_| {}) {
        RetainedOverscanOutcome::Fallback {
            canvas,
            reason: RetainedPaintError::StickyPosition,
            discarded_tiles: 0,
        } => assert_same_canvas(&canvas, &expected),
        _ => panic!("a phase-hoisted sticky box missing from its raw plan must fall back"),
    }
}

#[test]
fn overscan_rejects_inline_fixed_background_hoisted_through_hidden_flow_ancestor() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #hidden{visibility:hidden;height:1px}\
        #owner{visibility:visible;position:absolute;left:4px;top:44px;width:24px;height:16px}\
        span{font-size:12px;color:red;background-image:linear-gradient(red,red);\
        background-attachment:fixed}\
        </style><div id=hidden><div id=owner><span>MMMM</span></div></div>";
    let expected = ordinary_fixture(html, (0.0, 32.0), |_| {});
    assert!(
        expected
            .pixels()
            .chunks_exact(4)
            .any(|pixel| pixel[0] > pixel[1] && pixel[0] > pixel[2])
    );
    let control = ordinary_fixture(&html.replace("<span>MMMM</span>", ""), (0.0, 32.0), |_| {});
    assert_ne!(expected.pixels(), control.pixels());
    match overscan_fixture(html, VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(), |_| {}) {
        RetainedOverscanOutcome::Fallback {
            canvas,
            reason: RetainedPaintError::FixedBackground,
            discarded_tiles: 0,
        } => assert_same_canvas(&canvas, &expected),
        _ => panic!("a phase-hoisted inline fixed background must fall back before capture"),
    }
}

#[test]
fn overscan_rejects_top_layer_hoisted_through_hidden_flow_ancestor() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #hidden{visibility:hidden;height:1px}\
        #top{display:block;visibility:visible;position:fixed;inset:auto;left:8px;top:12px;width:8px;height:8px;\
        border:0;background:red;transform:none}\
        </style><div id=hidden><div id=top popover></div></div>";
    let setup = |document: &NodeHandle| {
        let top = document.query_selector("#top").unwrap();
        assert!(top.set_popover_open(true).is_some());
        assert!(top.top_layer_order().is_some());
    };
    let expected = ordinary_fixture(html, (0.0, 32.0), setup);
    assert_eq!(expected.pixel(12, 16), Some(Color::rgb(255, 0, 0)));
    let control = ordinary_fixture(
        &html.replace("background:red", "background:transparent"),
        (0.0, 32.0),
        setup,
    );
    assert_ne!(expected.pixels(), control.pixels());
    match overscan_fixture(html, VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(), setup) {
        RetainedOverscanOutcome::Fallback {
            canvas,
            reason: RetainedPaintError::TopLayer,
            discarded_tiles: 0,
        } => assert_same_canvas(&canvas, &expected),
        _ => panic!("a phase-hoisted top-layer box must fall back before capture"),
    }
}

#[test]
fn overscan_rejects_atomic_fixed_background_hoisted_through_hidden_flow_ancestor() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #hidden{visibility:hidden;height:1px}\
        #owner{visibility:visible;position:absolute;left:4px;top:44px;width:24px;height:16px}\
        #atomic{display:inline-block;width:16px;height:12px;\
        background-image:linear-gradient(red,red);background-attachment:fixed}\
        </style><div id=hidden><div id=owner><span id=atomic></span></div></div>";
    let expected = ordinary_fixture(html, (0.0, 32.0), |_| {});
    let center = expected.pixel(12, 16).unwrap();
    assert!(center.r > center.g && center.r > center.b);
    let control = ordinary_fixture(
        &html.replace("<span id=atomic></span>", ""),
        (0.0, 32.0),
        |_| {},
    );
    assert_ne!(expected.pixels(), control.pixels());
    match overscan_fixture(html, VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(), |_| {}) {
        RetainedOverscanOutcome::Fallback {
            canvas,
            reason: RetainedPaintError::FixedBackground,
            discarded_tiles: 0,
        } => assert_same_canvas(&canvas, &expected),
        _ => panic!("a phase-hoisted atomic fixed background must fall back before capture"),
    }
}

#[test]
fn overscan_rejects_fixed_root_hoisted_through_skipped_contents() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #skip{content-visibility:hidden}\
        #fixed{position:fixed;left:8px;top:44px;width:8px;height:8px;background:red}\
        </style><div id=skip><div id=fixed></div></div>";
    let expected = paint_forced_skipped_source_fixture(html, 32.0);
    assert_eq!(expected.pixel(12, 16), Some(Color::rgb(255, 0, 0)));
    let control =
        paint_forced_skipped_source_fixture(&html.replace("<div id=fixed></div>", ""), 32.0);
    assert_ne!(expected.pixels(), control.pixels());

    let (layout, mut resolver) = build_forced_skipped_source_fixture(html);
    let shadow_masks = super::border::FrameShadowMaskCache::new(false);
    match paint_layout_with_vertical_overscan(
        &layout,
        &mut resolver,
        viewport(),
        VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(),
        &[],
        None,
        &[],
        &std::collections::HashMap::new(),
        &shadow_masks,
    ) {
        RetainedOverscanOutcome::Fallback {
            canvas,
            reason: RetainedPaintError::TraversalMismatch,
            discarded_tiles: 0,
        } => assert_same_canvas(&canvas, &expected),
        _ => panic!("a fixed root hoisted through skipped contents must fall back"),
    }
}

#[test]
fn overscan_barriers_reject_filters_pruned_from_the_dom_scan() {
    for (barrier, local_scroll) in [
        ("transform:translateX(1px)", false),
        ("opacity:.8", false),
        ("contain:paint", false),
        ("overflow:hidden", true),
    ] {
        let html = format!(
            "<style>*{{margin:0;padding:0}}html{{background:white}}body{{height:160px}}\
             #barrier{{position:relative;width:32px;height:80px;{barrier}}}\
             #hidden{{visibility:hidden;height:1px}}\
             #effect{{visibility:visible;position:absolute;left:8px;top:44px;\
             width:8px;height:8px;background:red;filter:blur(4px)}}\
             </style><div id=barrier><div id=hidden><div id=effect></div></div></div>"
        );
        let setup = |document: &NodeHandle| {
            if local_scroll {
                document
                    .query_selector("#barrier")
                    .unwrap()
                    .set_scroll_offset(0.0, 1.0);
            }
        };
        let expected = ordinary_fixture(&html, (0.0, 32.0), setup);
        let center = expected.pixel(12, 16).unwrap();
        assert!(center.r > center.g && center.r > center.b, "{barrier}");
        match overscan_fixture(&html, VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(), setup) {
            RetainedOverscanOutcome::Fallback {
                canvas,
                reason: RetainedPaintError::ViewportEdgeFilter,
                discarded_tiles: 0,
            } => assert_same_canvas(&canvas, &expected),
            _ => panic!("a source below an unsupported barrier must fall back: {barrier}"),
        }
    }
}

#[test]
fn overscan_rejects_positioned_filter_hoisted_through_skipped_contents() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #skip{content-visibility:hidden;height:1px}\
        #effect{position:absolute;left:8px;top:44px;width:8px;height:8px;\
        background:red;filter:blur(4px)}\
        </style><div id=skip><div id=effect></div></div>";
    let document = TreeBuilder::parse(html).document();
    let skipped = document.query_selector("#skip").unwrap();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }
    let (layout, report) = crate::layout::layout_tree_with_content_visibility(
        &document,
        &mut resolver,
        viewport(),
        crate::layout::ContentVisibilityLayoutInput {
            visible_rect: viewport(),
            forced_nodes: std::collections::HashSet::from([skipped.identity()]),
            ..crate::layout::ContentVisibilityLayoutInput::default()
        },
    );
    assert!(report.skipped_nodes.contains(&skipped.identity()));
    let layout = layout.unwrap();
    let mut pending = vec![&layout];
    let skipped_layout = loop {
        let current = pending.pop().expect("skipped node must remain in layout");
        if current.node == skipped {
            break current;
        }
        pending.extend(current.children.iter());
    };
    assert!(skipped_layout.content_visibility_contents_skipped);
    assert!(!skipped_layout.children.is_empty());

    let mut adjusted = layout.clone();
    apply_scroll_offsets(&mut adjusted, &mut resolver, viewport(), (0.0, 32.0));
    let expected =
        paint_layout_with_web_fonts(&adjusted, &mut resolver, viewport(), Vec::new(), None);
    let center = expected.pixel(12, 16).unwrap();
    assert!(center.r > center.g && center.r > center.b);
    let shadow_masks = super::border::FrameShadowMaskCache::new(false);
    match paint_layout_with_vertical_overscan(
        &layout,
        &mut resolver,
        viewport(),
        VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(),
        &[],
        None,
        &[],
        &std::collections::HashMap::new(),
        &shadow_masks,
    ) {
        RetainedOverscanOutcome::Fallback {
            canvas,
            reason: RetainedPaintError::ViewportEdgeFilter,
            discarded_tiles: 0,
        } => assert_same_canvas(&canvas, &expected),
        _ => panic!("a source hoisted through skipped contents must fail before capture"),
    }
}

#[test]
fn overscan_positioned_filter_replays_multiple_phase_owner_clips() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #outer{position:relative;margin:17px 0 0 1px;width:29px;height:22px;overflow:hidden}\
        #inner{position:relative;margin:3px;width:25px;height:7px;overflow:hidden}\
        #effect{position:absolute;left:7px;top:6px;width:24px;height:19px;\
        background:rgba(255,0,0,.65);filter:blur(3px)}\
        </style><div id=outer><div id=inner><div id=effect></div></div></div>";
    let frame = expect_overscan(overscan_fixture(
        html,
        VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(),
        |_| {},
    ));
    assert_eq!(frame.runs().len(), 3);
    assert!(matches!(
        frame.runs()[1].identity(),
        RetainedPaintRunIdentity::FilterSource(_)
    ));
    assert_eq!(frame.runs()[1].filter_source_sample_count(), Some(3));
    for y in 0..=64 {
        assert_same_canvas(
            &frame.compose_integer((0, y)).unwrap(),
            &ordinary_fixture(html, (0.0, y as f32), |_| {}),
        );
    }
}

#[test]
fn overscan_phase_walk_orders_normal_source_before_dom_earlier_positioned_source() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #owner{position:relative;margin-top:17px;width:30px;height:24px;overflow:hidden}\
        #positioned{position:absolute;z-index:auto;left:3px;top:5px;width:24px;height:10px;\
        background:rgba(255,0,0,.7);filter:blur(2px)}\
        #before,#after{height:3px;background:rgba(0,0,255,.4)}\
        #normal{margin-left:6px;width:22px;height:8px;\
        background:rgba(0,255,0,.7);filter:blur(2px)}\
        </style><div id=owner><div id=positioned></div><div id=before></div>\
        <div id=normal></div><div id=after></div></div>";
    let document = TreeBuilder::parse(html).document();
    let positioned = document.query_selector("#positioned").unwrap().identity();
    let normal = document.query_selector("#normal").unwrap().identity();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }
    let layout = layout_tree(&document, &mut resolver, viewport()).unwrap();
    let shadow_masks = super::border::FrameShadowMaskCache::new(false);
    let frame = expect_overscan(paint_layout_with_vertical_overscan(
        &layout,
        &mut resolver,
        viewport(),
        VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(),
        &[],
        None,
        &[],
        &std::collections::HashMap::new(),
        &shadow_masks,
    ));
    assert_eq!(
        frame
            .runs()
            .iter()
            .map(|run| run.identity())
            .collect::<Vec<_>>(),
        [
            RetainedPaintRunIdentity::Document(0),
            RetainedPaintRunIdentity::FilterSource(normal),
            RetainedPaintRunIdentity::Document(1),
            RetainedPaintRunIdentity::FilterSource(positioned),
            RetainedPaintRunIdentity::Document(2),
        ]
    );
    assert_eq!(frame.runs()[1].filter_source_sample_count(), Some(3));
    assert_eq!(frame.runs()[3].filter_source_sample_count(), Some(3));
    for y in 0..=64 {
        assert_eq!(
            frame.compose_integer((0, y)).unwrap().pixels(),
            ordinary_fixture(html, (0.0, y as f32), |_| {}).pixels(),
            "y={y}"
        );
    }
}

#[test]
fn overscan_no_clip_explicit_z_and_float_filter_phases_stay_fallback() {
    for phase in [
        "position:absolute;left:4px;top:40px;z-index:-1",
        "position:absolute;left:4px;top:40px;z-index:0",
        "position:absolute;left:4px;top:40px;z-index:1",
        "float:left;margin:40px 0 0 4px",
    ] {
        let html = format!(
            "<style>*{{margin:0;padding:0}}html{{background:white}}body{{height:160px}}\
             #effect{{{phase};width:16px;height:16px;background:red;filter:blur(2px)}}\
             </style><div id=effect></div>"
        );
        let expected = ordinary_fixture(&html, (0.0, 32.0), |_| {});
        assert!(
            expected
                .pixels()
                .chunks_exact(4)
                .any(|pixel| pixel[0] > pixel[1] && pixel[0] > pixel[2]),
            "source phase must contribute red pixels: {phase}"
        );
        match overscan_fixture(
            &html,
            VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(),
            |_| {},
        ) {
            RetainedOverscanOutcome::Fallback {
                canvas,
                reason: RetainedPaintError::ViewportEdgeFilter,
                discarded_tiles: 0,
            } => assert_same_canvas(&canvas, &expected),
            _ => panic!("unsupported source phase must fall back before capture: {phase}"),
        }
    }
}

#[test]
fn overscan_no_clip_unproven_phase_ancestors_stay_fallback() {
    let fixtures = [
        (
            "z-index:-1 ancestor",
            "<style>*{margin:0;padding:0}body{height:160px}\
             #phase{position:relative;z-index:-1;height:80px}\
             #effect{position:absolute;left:4px;top:40px;width:16px;height:16px;\
             background:red;filter:blur(2px)}</style>\
             <div id=phase><div id=effect></div></div>",
        ),
        (
            "z-index:0 ancestor",
            "<style>*{margin:0;padding:0}body{height:160px}\
             #phase{position:relative;z-index:0;height:80px}\
             #effect{position:absolute;left:4px;top:40px;width:16px;height:16px;\
             background:red;filter:blur(2px)}</style>\
             <div id=phase><div id=effect></div></div>",
        ),
        (
            "z-index:1 ancestor",
            "<style>*{margin:0;padding:0}body{height:160px}\
             #phase{position:relative;z-index:1;height:80px}\
             #effect{position:absolute;left:4px;top:40px;width:16px;height:16px;\
             background:red;filter:blur(2px)}</style>\
             <div id=phase><div id=effect></div></div>",
        ),
        (
            "float ancestor",
            "<style>*{margin:0;padding:0}body{height:160px}\
             #phase{float:left;width:32px;height:80px}\
             #effect{position:absolute;left:4px;top:40px;width:16px;height:16px;\
             background:red;filter:blur(2px)}</style>\
             <div id=phase><div id=effect></div></div>",
        ),
        (
            "line owner",
            "<style>*{margin:0;padding:0}body{height:160px}\
             #phase{position:relative;height:80px}\
             #effect{position:absolute;left:4px;top:40px;width:16px;height:16px;\
             background:red;filter:blur(2px)}</style>\
             <div id=phase><span>line</span><div id=effect></div></div>",
        ),
        (
            "marker owner",
            "<style>*{margin:0;padding:0}body{height:160px}\
             #phase{position:relative;height:80px;list-style:disc}\
             #effect{position:absolute;left:4px;top:40px;width:16px;height:16px;\
             background:red;filter:blur(2px)}</style>\
             <ul><li id=phase><div id=effect></div></li></ul>",
        ),
        (
            "pseudo source",
            "<style>*{margin:0;padding:0}body{height:160px}\
             #phase{position:relative;height:80px}\
             #phase::before{content:'';display:block;position:absolute;left:4px;top:40px;\
             width:16px;height:16px;background:red;filter:blur(2px)}</style>\
             <div id=phase></div>",
        ),
    ];
    for (label, html) in fixtures {
        let expected = ordinary_fixture(html, (0.0, 32.0), |_| {});
        assert!(
            expected
                .pixels()
                .chunks_exact(4)
                .any(|pixel| pixel[0] > pixel[1] && pixel[0] > pixel[2]),
            "source phase must contribute red pixels: {label}"
        );
        match overscan_fixture(html, VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(), |_| {}) {
            RetainedOverscanOutcome::Fallback {
                canvas,
                reason: RetainedPaintError::ViewportEdgeFilter,
                discarded_tiles: 0,
            } => assert_same_canvas(&canvas, &expected),
            _ => panic!("unproven source ancestor must fall back before capture: {label}"),
        }
    }
}

#[test]
fn overscan_source_filter_nested_axis_clips_match_fractional_geometry() {
    let html = "<style>*{margin:0;padding:0}body{height:160px}\
        #outer{margin-left:1.25px;width:29.5px;height:80px;padding:.25px 0 0 .5px;\
        overflow-x:clip;overflow-y:visible;border-radius:4px}\
        #inner{margin:18.5px 0 0 3.25px;width:20.5px;height:18.5px;\
        overflow-x:visible;overflow-y:clip;border-radius:4px}\
        #effect{width:32px;height:32px;background:rgba(255,0,0,.6);filter:blur(10px)}\
        </style><div id=outer><div id=inner><div id=effect></div></div></div>";
    assert_sticky_integer_pixels(html, 0, 3)
        .validate_compositor(1.0)
        .unwrap();
}

#[test]
fn overscan_source_filter_clipped_translucent_siblings_keep_painter_order() {
    let html = "<style>*{margin:0;padding:0}body{height:160px}\
        #clip{margin:18.5px 0 0 2.5px;width:27.25px;height:20.5px;\
        overflow:hidden;border-radius:4px}\
        #before{width:20px;height:10px;background:rgba(0,0,255,.55)}\
        #effect{margin:-6px 0 0 4px;width:28px;height:22px;\
        background:rgba(255,0,0,.55);filter:blur(10px)}\
        #after{margin:-8px 0 0 12px;width:16px;height:10px;background:rgba(0,255,0,.55)}\
        </style><div id=clip><div id=before></div><div id=effect></div><div id=after></div></div>";
    assert_sticky_integer_pixels(html, 0, 3)
        .validate_compositor(1.0)
        .unwrap();
}

#[test]
fn overscan_source_filter_phase_hoist_matches_canonical_and_element_scroll_falls_back() {
    let hoisted = "<style>*{margin:0;padding:0}body{height:160px}\
        #clip{width:24px;height:24px;overflow:hidden}\
        #middle{width:32px;height:32px}#effect{position:absolute;left:16px;top:16px;\
        width:24px;height:24px;background:red;filter:blur(10px)}\
        </style><div id=clip><div id=middle><div id=effect></div></div></div>";
    let scrolled = "<style>*{margin:0;padding:0}body{height:160px}\
        #clip{width:24px;height:24px;overflow:hidden}\
        #effect{width:40px;height:40px;background:red;filter:blur(10px)}\
        </style><div id=clip><div id=effect></div></div>";
    let scroll_setup = |document: &NodeHandle| {
        document
            .query_selector("#clip")
            .unwrap()
            .set_scroll_offset(1.0, 1.0);
    };
    let frame = expect_overscan(overscan_fixture(
        hoisted,
        VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(),
        |_| {},
    ));
    assert_eq!(frame.runs().len(), 3);
    assert!(matches!(
        frame.runs()[1].identity(),
        RetainedPaintRunIdentity::FilterSource(_)
    ));
    assert_eq!(frame.runs()[1].filter_source_sample_count(), Some(3));
    for y in 0..=64 {
        assert_eq!(
            frame.compose_integer((0, y)).unwrap().pixels(),
            ordinary_fixture(hoisted, (0.0, y as f32), |_| {}).pixels(),
            "hoisted y={y}"
        );
    }
    match overscan_fixture(
        scrolled,
        VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(),
        scroll_setup,
    ) {
        RetainedOverscanOutcome::Fallback {
            canvas,
            reason: RetainedPaintError::ViewportEdgeFilter,
            discarded_tiles: 0,
        } => assert_same_canvas(
            &canvas,
            &ordinary_fixture(scrolled, (0.0, 32.0), scroll_setup),
        ),
        _ => panic!("independently scrolled clips must fall back before capture"),
    }
}

fn nested_overflow_filter_fixture(depth: usize) -> String {
    let mut content = String::from(
        "<div id=effect style='width:40px;height:40px;background:red;filter:blur(2px)'></div>",
    );
    for _ in 0..depth {
        content = format!("<div class=clip>{content}</div>");
    }
    format!(
        "<style>*{{margin:0;padding:0}}body{{height:160px}}\
         .clip{{width:31px;height:31px;overflow:hidden}}</style>{content}"
    )
}

#[test]
fn overscan_source_filter_clip_chain_has_a_strict_sixteen_operation_limit() {
    let accepted = nested_overflow_filter_fixture(16);
    assert_sticky_integer_pixels(&accepted, 0, 3)
        .validate_compositor(1.0)
        .unwrap();

    let rejected = nested_overflow_filter_fixture(17);
    match overscan_fixture(
        &rejected,
        VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(),
        |_| {},
    ) {
        RetainedOverscanOutcome::Fallback {
            canvas,
            reason: RetainedPaintError::Capacity,
            discarded_tiles: 0,
        } => assert_same_canvas(&canvas, &ordinary_fixture(&rejected, (0.0, 32.0), |_| {})),
        _ => panic!("a seventeenth retained clip must fail before sample collection"),
    }
}

#[test]
fn overscan_clipped_filter_source_preserves_visible_animation_metadata() {
    let document = TreeBuilder::parse(
        "<style>*{margin:0;padding:0}body{height:160px}\
         #clip{margin:20px 0 0 4px;width:20px;height:12px;overflow:hidden}\
         #effect{width:24px;height:16px;filter:blur(2px)}\
         iframe{display:block;width:8px;height:8px;border:0}</style>\
         <div id=clip><div id=effect><iframe id=child></iframe></div></div>",
    )
    .document();
    let iframe = document.query_selector("#child").unwrap();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }
    let layout = layout_tree(&document, &mut resolver, viewport()).unwrap();
    let image_frame = |red| ImageFrame {
        image: Image::new(1, 1, vec![red, 0, 0, 255]).unwrap(),
        delay_ms: 100,
        disposal: GifDisposal::Keep,
    };
    let animation = Arc::new(ImageAnimation {
        frames: vec![image_frame(255), image_frame(128)],
        delays_ms: vec![100, 100],
        duration_ms: 200,
        plays: None,
    });
    let snapshots = std::collections::HashMap::from([(
        iframe.identity(),
        animation.playback_image(91, "test:clipped-filter", 0, 0),
    )]);
    let shadow_masks = super::border::FrameShadowMaskCache::new(false);
    let retained = expect_overscan(paint_layout_with_vertical_overscan(
        &layout,
        &mut resolver,
        viewport(),
        VerticalOverscanPlan::new(0, 0, 3, 0).unwrap(),
        &[],
        None,
        &[],
        &snapshots,
        &shadow_masks,
    ));
    let visible = retained.compose_integer((0, 0)).unwrap();
    assert_eq!(visible.image_playbacks().len(), 1);
    assert_eq!(visible.image_playbacks()[0].document, 91);
    assert!(
        retained
            .compose_integer((0, 64))
            .unwrap()
            .image_playbacks()
            .is_empty()
    );
}

#[test]
fn overscan_source_filter_unknown_effects_and_clips_keep_exact_fallback() {
    for declaration in [
        "opacity:.8",
        "filter:brightness(.8)",
        "mask-image:linear-gradient(black,transparent)",
        "clip-path:circle(40%)",
        "contain:paint",
        "transform:translate(0px)",
    ] {
        let html = format!(
            "<style>*{{margin:0;padding:0}}html{{background:white}}body{{height:160px}}\
             #ancestor{{{declaration}}}#effect{{position:absolute;left:4px;top:24px;width:24px;height:24px;\
             background:red;filter:blur(10px)}}</style><div id=ancestor><div id=effect></div></div>"
        );
        match overscan_fixture(
            &html,
            VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(),
            |_| {},
        ) {
            RetainedOverscanOutcome::Fallback {
                canvas,
                reason: RetainedPaintError::ViewportEdgeFilter,
                discarded_tiles: 0,
            } => assert_same_canvas(&canvas, &ordinary_fixture(&html, (0.0, 32.0), |_| {})),
            _ => panic!("unknown ancestor must retain ordinary fallback: {declaration}"),
        }
    }
}

#[test]
fn overscan_source_filter_nested_spatial_and_fixed_children_are_rejected() {
    for child in [
        "<div style='width:8px;height:8px;background:blue;filter:blur(2px)'></div>",
        "<div style='position:fixed;top:4px;width:8px;height:8px;background:blue'></div>",
    ] {
        let html = format!(
            "<style>*{{margin:0;padding:0}}body{{height:160px}}#effect{{width:16px;height:16px;filter:blur(4px)}}</style><div id=effect>{child}</div>"
        );
        match overscan_fixture(
            &html,
            VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(),
            |_| {},
        ) {
            RetainedOverscanOutcome::Fallback {
                canvas,
                reason,
                discarded_tiles: 0,
            } => {
                assert!(matches!(
                    reason,
                    RetainedPaintError::ViewportEdgeFilter | RetainedPaintError::CrossSpaceEffect
                ));
                assert_same_canvas(&canvas, &ordinary_fixture(&html, (0.0, 32.0), |_| {}));
            }
            _ => panic!("unsupported source descendant must fall back"),
        }
    }
}

#[test]
fn overscan_source_filter_run_limit_is_preflighted_without_discarding_a_tile() {
    let groups = "<div class=effect></div>".repeat(32);
    let html = format!(
        "<style>*{{margin:0;padding:0}}body{{height:160px}}.effect{{position:absolute;width:8px;height:8px;background:red;filter:blur(2px)}}</style>{groups}"
    );
    match overscan_fixture(
        &html,
        VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(),
        |_| {},
    ) {
        RetainedOverscanOutcome::Fallback {
            canvas,
            reason: RetainedPaintError::Capacity,
            discarded_tiles: 0,
        } => assert_same_canvas(&canvas, &ordinary_fixture(&html, (0.0, 32.0), |_| {})),
        _ => panic!("source split run limit must be enforced before capture"),
    }
}

#[test]
fn overscan_source_filter_actual_nico_fractional_geometry_is_not_rounded_to_css_pixels() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:3000px}\
        #effect{position:absolute;left:22px;top:2085.5px;width:209.33333px;height:126.5px;\
        background:red;filter:blur(10px)}</style><div id=effect></div>";
    // 32px fixture viewport and the same half-pixel Y phase as the diagnostic.
    assert_sticky_integer_pixels(html, 2048, 3);
}

#[test]
fn overscan_source_filter_fractional_scroll_keeps_the_owned_bilinear_source_contract() {
    let html = "<style>*{margin:0;padding:0}body{height:160px}\
        #effect{position:absolute;left:4px;top:24px;width:24px;height:24px;background:red;filter:blur(2px)}\
        </style><div id=effect></div>";
    let frame = expect_overscan(overscan_fixture(
        html,
        VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(),
        |_| {},
    ));
    let actual = frame.compose_logical((0.0, 0.5), 1.0).unwrap();
    // This is an independent numeric expectation for the existing bilinear
    // raster-source contract, not a claim of ordinary fractional raster identity.
    let mut source = Canvas::new(28, 11);
    source.fill_rect(
        Rect {
            x: 2.0,
            y: 2.0,
            width: 24.0,
            height: 1.0,
        },
        Color::rgba(255, 0, 0, 128),
    );
    source.fill_rect(
        Rect {
            x: 2.0,
            y: 3.0,
            width: 24.0,
            height: 8.0,
        },
        Color::rgb(255, 0, 0),
    );
    super::apply_filters(
        &mut source,
        &crate::css::parse_filter_list("blur(2px)").unwrap(),
    );
    let mut expected = Canvas::new(32, 32);
    expected.draw_image(&source.into_image(), 2.0, 21.0);
    assert_same_canvas(&actual, &expected);
}

#[test]
fn overscan_source_filter_large_offset_and_fractional_padding_matches_canonical_geometry() {
    let html = "<style>*{margin:0;padding:0}body{height:1000200px}\
        #effect{position:absolute;left:4px;top:999999px;width:16px;height:16px;\
        padding-top:.7px;border-top:.333333px solid transparent;background:red;filter:blur(10px)}\
        </style><div id=effect></div>";
    assert_sticky_integer_pixels(html, 999936, 3);
}
fn atomic_source_overscan(html: &str, selectors: &[&str]) -> (Vec<usize>, RetainedOverscanFrame) {
    let document = TreeBuilder::parse(html).document();
    let source_ids = selectors
        .iter()
        .map(|selector| document.query_selector(selector).unwrap().identity())
        .collect::<Vec<_>>();
    let mut resolver = StyleResolver::new();
    for stylesheet in extract_author_stylesheets(&document, None).unwrap() {
        resolver.add_stylesheet(Origin::Author, parse_stylesheet_forgiving(&stylesheet));
    }
    let layout = layout_tree(&document, &mut resolver, viewport()).unwrap();
    let shadow_masks = super::border::FrameShadowMaskCache::new(false);
    let frame = expect_overscan(paint_layout_with_vertical_overscan(
        &layout,
        &mut resolver,
        viewport(),
        VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(),
        &[],
        None,
        &[],
        &std::collections::HashMap::new(),
        &shadow_masks,
    ));
    (source_ids, frame)
}

fn selected_atomic_invocation_count(layout: &LayoutBox, node: &NodeHandle) -> usize {
    let own = layout
        .lines
        .iter()
        .flat_map(|line| {
            line.text_overflow
                .as_ref()
                .map_or(line.fragments.as_slice(), |overflow| {
                    overflow.fragments.as_slice()
                })
        })
        .filter(|fragment| {
            fragment.node == *node
                && fragment.style.visibility != crate::layout::Visibility::Hidden
                && matches!(fragment.content, InlineFragmentContent::AtomicInline(_))
                && layout.children.iter().any(|child| child.node == *node)
        })
        .count();
    own + layout
        .children
        .iter()
        .map(|child| selected_atomic_invocation_count(child, node))
        .sum::<usize>()
}

fn assert_unique_atomic_invocations(html: &str, selectors: &[&str]) {
    let (layout, _) = build_raw_fixture(html, |_| {});
    for selector in selectors {
        let node = layout.node.query_selector(selector).unwrap();
        assert_eq!(
            selected_atomic_invocation_count(&layout, &node),
            1,
            "{selector} must have one selected, visible, direct-child invocation"
        );
    }
}

fn assert_atomic_source_run_identities(source_ids: &[usize], frame: &RetainedOverscanFrame) {
    let mut expected = vec![RetainedPaintRunIdentity::Document(0)];
    for (index, source) in source_ids.iter().copied().enumerate() {
        expected.push(RetainedPaintRunIdentity::FilterSource(source));
        expected.push(RetainedPaintRunIdentity::Document(index + 1));
    }
    assert_eq!(
        frame
            .runs()
            .iter()
            .map(|run| run.identity())
            .collect::<Vec<_>>(),
        expected
    );
    for run in frame
        .runs()
        .iter()
        .filter(|run| matches!(run.identity(), RetainedPaintRunIdentity::FilterSource(_)))
    {
        assert_eq!(run.filter_source_sample_count(), Some(3));
    }
}

fn assert_atomic_source_runs(html: &str, source_ids: &[usize], frame: &RetainedOverscanFrame) {
    assert_atomic_source_run_identities(source_ids, frame);
    for y in 0..=64 {
        let actual = frame.compose_integer((0, y)).unwrap();
        let expected = ordinary_fixture(html, (0.0, y as f32), |_| {});
        assert_eq!(
            (actual.width(), actual.height()),
            (expected.width(), expected.height()),
            "atomic source y={y}: dimensions"
        );
        let width = actual.width() as usize;
        assert!(
            actual.pixels() == expected.pixels(),
            "atomic source y={y}: first differing RGBA pixel {:?}",
            actual
                .pixels()
                .chunks_exact(4)
                .zip(expected.pixels().chunks_exact(4))
                .enumerate()
                .find(|(_, (actual, expected))| actual != expected)
                .map(|(index, (actual, expected))| {
                    (index % width, index / width, actual, expected)
                })
        );
    }
}

#[test]
fn overscan_atomic_inline_source_preserves_text_shadow_and_decoration_runs() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #owner{margin-top:20px;width:32px;height:20px;color:rgba(0,0,180,.65);\
        text-shadow:1px 1px rgba(0,160,0,.55);text-decoration:underline}\
        #atomic{display:inline-block;width:11px;height:12px;vertical-align:middle;\
        background:rgba(255,0,0,.65);filter:blur(3px)}\
        </style><div id=owner>A<span id=atomic></span>B</div>";
    let ordinary = ordinary_fixture(html, (0.0, 0.0), |_| {});
    let no_filter = ordinary_fixture(
        &html.replace("filter:blur(3px)", "filter:none"),
        (0.0, 0.0),
        |_| {},
    );
    assert_ne!(ordinary.pixels(), no_filter.pixels());
    assert_unique_atomic_invocations(html, &["#atomic"]);
    let (sources, frame) = atomic_source_overscan(html, &["#atomic"]);
    assert_atomic_source_runs(html, &sources, &frame);
}

#[test]
fn overscan_atomic_inline_descendant_source_preserves_text_phase() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #owner{margin-top:20px;width:32px;height:20px;color:rgba(0,0,180,.65);\
        text-shadow:1px 1px rgba(0,160,0,.55);text-decoration:underline}\
        #atomic{display:inline-block;width:16px;height:14px;vertical-align:middle}\
        #effect{display:block;width:11px;height:10px;background:rgba(255,0,0,.65);\
        filter:blur(3px)}\
        </style><div id=owner>A<span id=atomic><span id=effect></span></span>B</div>";
    let ordinary = ordinary_fixture(html, (0.0, 0.0), |_| {});
    let no_filter = ordinary_fixture(
        &html.replace("filter:blur(3px)", "filter:none"),
        (0.0, 0.0),
        |_| {},
    );
    assert_ne!(ordinary.pixels(), no_filter.pixels());
    assert_unique_atomic_invocations(html, &["#atomic"]);
    let (sources, frame) = atomic_source_overscan(html, &["#effect"]);
    assert_atomic_source_runs(html, &sources, &frame);
}

#[test]
fn overscan_two_atomic_inline_sources_follow_fragment_text_order() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #owner{margin-top:20px;width:32px;height:18px;color:rgba(0,0,0,.55)}\
        .atomic{display:inline-block;width:8px;height:10px;vertical-align:middle;filter:blur(2px)}\
        #a{background:rgba(255,0,0,.65)}#b{background:rgba(0,0,255,.65)}\
        </style><div id=owner>L<span class=atomic id=a></span>M<span class=atomic id=b></span>R</div>";
    let ordinary = ordinary_fixture(html, (0.0, 0.0), |_| {});
    let no_filters = ordinary_fixture(
        &html.replace("filter:blur(2px)", "filter:none"),
        (0.0, 0.0),
        |_| {},
    );
    assert_ne!(ordinary.pixels(), no_filters.pixels());
    assert_unique_atomic_invocations(html, &["#a", "#b"]);
    let (sources, frame) = atomic_source_overscan(html, &["#a", "#b"]);
    assert_atomic_source_runs(html, &sources, &frame);
}

#[test]
fn overscan_atomic_inline_source_replays_owner_clip_and_empty_sample_hook() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #owner{margin:20px 0 0 4px;width:20px;height:20px;overflow:hidden;white-space:nowrap}\
        #atomic{display:inline-block;width:28px;height:14px;background:rgba(255,0,0,.7);\
        filter:blur(6px)}\
        </style><div id=owner><span id=atomic></span></div>";
    let top = ordinary_fixture(html, (0.0, 0.0), |_| {});
    let middle = ordinary_fixture(html, (0.0, 32.0), |_| {});
    let bottom = ordinary_fixture(html, (0.0, 64.0), |_| {});
    let has_red = |canvas: &Canvas| {
        canvas
            .pixels()
            .chunks_exact(4)
            .any(|pixel| pixel[0] > pixel[1] && pixel[0] > pixel[2])
    };
    assert!(has_red(&top));
    assert!(has_red(&middle));
    assert!(!has_red(&bottom));
    assert_unique_atomic_invocations(html, &["#atomic"]);
    let (sources, frame) = atomic_source_overscan(html, &["#atomic"]);
    assert_atomic_source_runs(html, &sources, &frame);
}

#[test]
fn overscan_atomic_inline_source_uses_selected_text_overflow_order() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #owner{margin-top:20px;width:32px;height:18px;color:rgba(0,0,0,.55)}\
        .atomic{display:inline-block;width:9px;height:11px;filter:blur(2px)}\
        #a{background:rgba(255,0,0,.65)}#b{background:rgba(0,0,255,.65)}\
        </style><div id=owner><span class=atomic id=a></span>M<span class=atomic id=b></span></div>";
    let (source_ids, outcome) =
        overscan_mutated_atomic_fixture(html, &["#b", "#a"], reverse_selected_atomic_fragments);
    let frame = expect_overscan(outcome);
    assert_atomic_source_run_identities(&source_ids, &frame);
    for y in 0..=64 {
        assert_eq!(
            frame.compose_integer((0, y)).unwrap().pixels(),
            paint_mutated_atomic_fixture(html, y as f32, reverse_selected_atomic_fragments)
                .pixels(),
            "selected overflow order y={y}"
        );
    }
}

fn atomic_line_owner_mut(layout: &mut LayoutBox) -> Option<&mut LayoutBox> {
    if layout.lines.iter().any(|line| {
        line.fragments
            .iter()
            .any(|fragment| matches!(fragment.content, InlineFragmentContent::AtomicInline(_)))
    }) {
        return Some(layout);
    }
    layout.children.iter_mut().find_map(atomic_line_owner_mut)
}

fn duplicate_first_atomic_fragment(layout: &mut LayoutBox) {
    let owner = atomic_line_owner_mut(layout).expect("fixture must have an atomic line owner");
    let line = owner
        .lines
        .iter_mut()
        .find(|line| {
            line.fragments
                .iter()
                .any(|fragment| matches!(fragment.content, InlineFragmentContent::AtomicInline(_)))
        })
        .unwrap();
    let index = line
        .fragments
        .iter()
        .position(|fragment| matches!(fragment.content, InlineFragmentContent::AtomicInline(_)))
        .unwrap();
    let duplicate = line.fragments[index].clone();
    line.fragments.insert(index + 1, duplicate);
}

fn first_atomic_fragment_mut(layout: &mut LayoutBox) -> &mut crate::layout::InlineFragment {
    let owner = atomic_line_owner_mut(layout).expect("fixture must have an atomic line owner");
    owner
        .lines
        .iter_mut()
        .flat_map(|line| &mut line.fragments)
        .find(|fragment| matches!(fragment.content, InlineFragmentContent::AtomicInline(_)))
        .unwrap()
}

fn reverse_selected_atomic_fragments(layout: &mut LayoutBox) {
    let owner = atomic_line_owner_mut(layout).expect("fixture must have an atomic line owner");
    let line = owner
        .lines
        .iter_mut()
        .find(|line| {
            line.fragments
                .iter()
                .filter(|fragment| {
                    matches!(fragment.content, InlineFragmentContent::AtomicInline(_))
                })
                .count()
                == 2
        })
        .unwrap();
    let mut selected = line
        .fragments
        .iter()
        .filter(|fragment| matches!(fragment.content, InlineFragmentContent::AtomicInline(_)))
        .cloned()
        .collect::<Vec<_>>();
    selected.reverse();
    line.text_overflow = Some(crate::layout::TextOverflowPaint {
        fragments: selected,
    });
}

fn hide_selected_atomic_fragment(layout: &mut LayoutBox) {
    let selected = first_atomic_fragment_mut(layout).clone();
    let mut selected = selected;
    selected.style.visibility = crate::layout::Visibility::Hidden;
    let owner = atomic_line_owner_mut(layout).unwrap();
    owner.lines[0].text_overflow = Some(crate::layout::TextOverflowPaint {
        fragments: vec![selected],
    });
}

fn select_missing_atomic_child(layout: &mut LayoutBox) {
    let missing = layout.node.query_selector("#effect").unwrap();
    let mut selected = first_atomic_fragment_mut(layout).clone();
    selected.node = missing;
    let owner = atomic_line_owner_mut(layout).unwrap();
    owner.lines[0].text_overflow = Some(crate::layout::TextOverflowPaint {
        fragments: vec![selected],
    });
}

fn insert_same_node_atomic_child_first(layout: &mut LayoutBox) {
    let owner = atomic_line_owner_mut(layout).expect("fixture must have an atomic line owner");
    let atomic = owner
        .lines
        .iter()
        .flat_map(|line| &line.fragments)
        .find(|fragment| matches!(fragment.content, InlineFragmentContent::AtomicInline(_)))
        .unwrap()
        .node
        .clone();
    let index = owner
        .children
        .iter()
        .position(|child| child.node == atomic)
        .unwrap();
    let mut first = owner.children[index].clone();
    first.dimensions.content.x += 9.0;
    owner.children.insert(index, first);
}

fn paint_mutated_atomic_fixture(html: &str, scroll_y: f32, mutate: fn(&mut LayoutBox)) -> Canvas {
    let (mut layout, mut resolver) = build_raw_fixture(html, |_| {});
    mutate(&mut layout);
    apply_scroll_offsets(&mut layout, &mut resolver, viewport(), (0.0, scroll_y));
    paint_layout_with_web_fonts(&layout, &mut resolver, viewport(), Vec::new(), None)
}

fn overscan_mutated_atomic_fixture(
    html: &str,
    selectors: &[&str],
    mutate: fn(&mut LayoutBox),
) -> (Vec<usize>, RetainedOverscanOutcome) {
    let (mut layout, mut resolver) = build_raw_fixture(html, |_| {});
    let source_ids = selectors
        .iter()
        .map(|selector| layout.node.query_selector(selector).unwrap().identity())
        .collect();
    mutate(&mut layout);
    let shadow_masks = super::border::FrameShadowMaskCache::new(false);
    (
        source_ids,
        paint_layout_with_vertical_overscan(
            &layout,
            &mut resolver,
            viewport(),
            VerticalOverscanPlan::new(0, 0, 3, 1).unwrap(),
            &[],
            None,
            &[],
            &std::collections::HashMap::new(),
            &shadow_masks,
        ),
    )
}

#[test]
fn overscan_hidden_selected_atomic_fragment_is_not_a_source() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #owner{margin-top:44px;width:28px;height:16px}#atomic{display:inline-block;\
        width:14px;height:12px;background:rgba(255,0,0,.55);filter:blur(2px)}\
        </style><div id=owner><span id=atomic></span></div>";
    let expected = paint_mutated_atomic_fixture(html, 32.0, hide_selected_atomic_fragment);
    assert_ne!(
        expected.pixels(),
        ordinary_fixture(html, (0.0, 32.0), |_| {}).pixels()
    );
    let (_, outcome) =
        overscan_mutated_atomic_fixture(html, &[] as &[&str], hide_selected_atomic_fragment);
    let frame = expect_overscan(outcome);
    assert_eq!(
        frame
            .runs()
            .iter()
            .map(|run| run.identity())
            .collect::<Vec<_>>(),
        vec![RetainedPaintRunIdentity::Document(0)]
    );
    for y in 0..=64 {
        assert_eq!(
            frame.compose_integer((0, y)).unwrap().pixels(),
            paint_mutated_atomic_fixture(html, y as f32, hide_selected_atomic_fragment).pixels(),
            "hidden selected fragment y={y}"
        );
    }
}

#[test]
fn overscan_atomic_inline_uses_the_first_same_node_direct_child() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #owner{margin-top:44px;width:32px;height:16px}#atomic{display:inline-block;\
        width:10px;height:12px;background:rgba(255,0,0,.55);filter:blur(2px)}\
        </style><div id=owner><span id=atomic></span></div>";
    let shifted = paint_mutated_atomic_fixture(html, 32.0, insert_same_node_atomic_child_first);
    assert_ne!(
        shifted.pixels(),
        ordinary_fixture(html, (0.0, 32.0), |_| {}).pixels()
    );
    let (sources, outcome) =
        overscan_mutated_atomic_fixture(html, &["#atomic"], insert_same_node_atomic_child_first);
    let frame = expect_overscan(outcome);
    assert_atomic_source_run_identities(&sources, &frame);
    for y in 0..=64 {
        assert_eq!(
            frame.compose_integer((0, y)).unwrap().pixels(),
            paint_mutated_atomic_fixture(html, y as f32, insert_same_node_atomic_child_first)
                .pixels(),
            "first same-node child y={y}"
        );
    }
}

#[test]
fn overscan_duplicate_and_missing_atomic_invocations_fall_back_before_collection() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #owner{margin-top:44px;width:28px;height:16px}#atomic{display:inline-block;\
        width:14px;height:12px}#effect{display:block;width:12px;height:10px;\
        background:rgba(255,0,0,.55);filter:blur(2px)}\
        </style><div id=owner><span id=atomic><span id=effect></span></span></div>";
    let ordinary = ordinary_fixture(html, (0.0, 32.0), |_| {});
    for (label, mutate) in [
        (
            "duplicate",
            duplicate_first_atomic_fragment as fn(&mut LayoutBox),
        ),
        ("missing", select_missing_atomic_child as fn(&mut LayoutBox)),
    ] {
        let expected = paint_mutated_atomic_fixture(html, 32.0, mutate);
        assert_ne!(expected.pixels(), ordinary.pixels(), "{label}");
        match overscan_mutated_atomic_fixture(html, &["#effect"], mutate).1 {
            RetainedOverscanOutcome::Fallback {
                canvas,
                reason: RetainedPaintError::TraversalMismatch,
                discarded_tiles: 0,
            } => assert_same_canvas(&canvas, &expected),
            _ => panic!("{label} atomic invocation must fail before collection"),
        }
    }
}

#[test]
fn retained_fixed_text_operations_preserve_backdrop_rounding() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #pin{position:fixed;left:4px;top:4px;width:24px;height:24px;\
        color:rgba(0,0,180,.65);letter-spacing:-5px;\
        text-shadow:1px 1px rgba(0,160,0,.55),2px 0 rgba(160,0,0,.4)}\
        </style><div id=pin>ABC<br>Q</div>";
    let expected = ordinary_fixture(html, (0.0, 0.0), |_| {});
    let no_shadows = ordinary_fixture(
        &html.replace(
            "text-shadow:1px 1px rgba(0,160,0,.55),2px 0 rgba(160,0,0,.4)",
            "text-shadow:none",
        ),
        (0.0, 0.0),
        |_| {},
    );
    assert!(expected.pixels() != no_shadows.pixels());
    for y in [0, 13, 32] {
        let frame = expect_extracted(retained_fixture(html, (0.0, y as f32), |_| {}));
        assert_eq!(frame.runs().len(), 3);
        assert_same_canvas(
            &frame.compose(),
            &ordinary_fixture(html, (0.0, y as f32), |_| {}),
        );
    }
}

#[test]
fn overscan_atomic_adjacent_opaque_primitives_keep_fractional_coverage() {
    let html = "<style>*{margin:0;padding:0}html{background:white}body{height:160px}\
        #owner{margin-top:20px;width:32px;height:8px;font-size:0;line-height:2px}\
        #source,#pair{display:inline-block;width:1px;vertical-align:top}\
        #source{height:1px;background:lime;filter:blur(1px)}#pair{height:2px}\
        #red,#blue{display:block;width:1px;height:1px}\
        #red{background:red}#blue{background:blue}\
        </style><div id=owner><span id=source></span><span id=pair>\
        <span id=red></span><span id=blue></span></span></div>";
    let ordinary = ordinary_fixture(html, (0.0, 0.0), |_| {});
    let (x, y) = (0..ordinary.height() - 1)
        .flat_map(|y| (0..ordinary.width()).map(move |x| (x, y)))
        .find(|&(x, y)| {
            ordinary.pixel(x, y) == Some(Color::rgb(255, 0, 0))
                && ordinary.pixel(x, y + 1) == Some(Color::rgb(0, 0, 255))
        })
        .expect("fixture must paint adjacent fully opaque red and blue pixels");
    assert_unique_atomic_invocations(html, &["#source", "#pair"]);
    let (sources, frame) = atomic_source_overscan(html, &["#source"]);
    assert_atomic_source_runs(html, &sources, &frame);
    let fractional = frame.compose_logical((0.0, 0.5), 1.0).unwrap();
    assert_eq!(
        fractional.pixel(x, y),
        Some(Color::rgb(128, 0, 128)),
        "adjacent opaque colors must retain full coverage after half-pixel sampling"
    );
}
