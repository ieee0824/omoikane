use super::*;

fn geometry(
    content: Rect,
    padding: crate::layout::EdgeSizes,
    border: crate::layout::EdgeSizes,
) -> BoundsNode {
    BoundsNode {
        dimensions: crate::layout::BoxDimensions {
            content,
            padding,
            border,
            ..crate::layout::BoxDimensions::default()
        },
        children: Vec::new(),
        padding: (0, 0, 0, 0),
        nodes: 1,
    }
}

#[test]
fn source_bounds_replay_translates_content_before_fractional_padding_and_border() {
    let mut geometry = geometry(
        Rect {
            x: 0.0,
            y: 1_000_000.0,
            width: 16.0,
            height: 16.0,
        },
        crate::layout::EdgeSizes {
            top: 0.7,
            ..Default::default()
        },
        crate::layout::EdgeSizes {
            top: 1.0 / 3.0,
            ..Default::default()
        },
    );
    geometry.padding = (10, 10, 10, 10);
    let raw = geometry.at_scroll((0.0, 0.0));
    let replay = geometry.at_scroll((0.0, 999956.0));
    assert_eq!((raw.y - 999956.0).floor(), 33.0);
    assert_eq!(replay.y.floor(), 32.0);
    assert_eq!(replay.y, 32.966667);
}

#[test]
fn source_bounds_replay_adds_height_after_translation_at_actual_nico_phase() {
    for (y, height, scroll) in [
        (2085.5f32, 126.50001f32, 2085.0),
        (65536.125, 126.8751, 65535.0),
        (1048000.125, 0.87501, 1047900.0),
    ] {
        let geometry = geometry(
            Rect {
                x: 0.0,
                y,
                width: 16.0,
                height,
            },
            Default::default(),
            Default::default(),
        );
        let canonical = geometry.at_scroll((0.0, scroll as f64));
        let canonical_bottom = canonical.y + canonical.height;
        let raw = geometry.at_scroll((0.0, 0.0));
        let shortcut_bottom = (raw.y + raw.height) - scroll;
        assert_ne!(canonical_bottom.ceil(), shortcut_bottom.ceil());
    }
}

#[test]
fn source_filter_scratch_is_budgeted_before_publication() {
    let dimensions = SurfaceDimensions {
        width: 1280,
        height: 720,
        bytes: 1280 * 720 * 4,
    };
    let geometry = geometry(
        Rect {
            x: 0.0,
            y: 0.0,
            width: 1280.0,
            height: 720.0,
        },
        Default::default(),
        Default::default(),
    );
    let recipe = FilterRecipe {
        node: 1,
        bounds: geometry.at_scroll((0.0, 0.0)),
        geometry,
        clips: OwnedOverflowClipChain::default(),
        paint_owner_node_id: None,
        paint_ordinal: Some(0),
        paint_offset: Some(PaintOffsetKey::of(PaintOffset::default())),
        atomic_inline: None,
        filters: vec![crate::css::FilterFunction::Blur(10.0)],
        opacity: 1.0,
        active: true,
    };
    let scratch = recipe.scratch(dimensions).unwrap();
    let mut recipes = HashMap::new();
    recipes.insert(1, recipe);
    let sources = FilterSourcePlan {
        recipes,
        ordered_active: vec![1],
        skipped_by_clip_owner: HashMap::new(),
        raw_fixed: HashMap::new(),
        raw_viewport_fixed_count: 0,
        raw_sticky: HashSet::new(),
        viewport: Rect {
            x: 0.0,
            y: 0.0,
            width: 1280.0,
            height: 720.0,
        },
        scroll: (0, 0),
        source_bytes: 1280 * 720 * 4 * 3,
        scratch_bytes: scratch,
        metadata_bytes: 4096,
    };
    let plan = VerticalOverscanPlan::new(0, 0, 3, 0).unwrap();
    assert!(matches!(
        plan.resolve(sources.viewport, dimensions, 3, Some(&sources), None),
        Err(RetainedPaintError::Capacity)
    ));
    let without_scratch = FilterSourcePlan {
        scratch_bytes: 0,
        ..sources
    };
    assert!(
        plan.resolve(
            without_scratch.viewport,
            dimensions,
            3,
            Some(&without_scratch),
            None
        )
        .is_ok()
    );
}

#[test]
fn source_coverage_distinguishes_known_transparent_from_missing_pixels() {
    let geometry = geometry(
        Rect {
            x: 0.0,
            y: 0.0,
            width: 8.0,
            height: 8.0,
        },
        Default::default(),
        Default::default(),
    );
    let recipe = FilterRecipe {
        node: 1,
        bounds: geometry.at_scroll((0.0, 0.0)),
        geometry,
        clips: OwnedOverflowClipChain::default(),
        paint_owner_node_id: None,
        paint_ordinal: Some(0),
        paint_offset: Some(PaintOffsetKey::of(PaintOffset::default())),
        atomic_inline: None,
        filters: vec![crate::css::FilterFunction::Blur(1.0)],
        opacity: 1.0,
        active: true,
    };
    let layer = FilterSourceLayer {
        recipe,
        viewport: Rect {
            x: 0.0,
            y: 0.0,
            width: 32.0,
            height: 32.0,
        },
        samples: vec![SourceSample {
            coverage: PixelRect {
                x: 0,
                y: 0,
                width: 32,
                height: 32,
            },
            pixels: None,
        }],
    };
    assert_eq!(layer.pixel(8, 8), Some([0; 4]));
    assert_eq!(layer.pixel(8, 32), None);
    assert!(
        layer
            .validate(OverscanRasterRect {
                left: 0,
                top: 0,
                right: 32,
                bottom: 32
            })
            .is_ok()
    );
    assert!(matches!(
        layer.validate(OverscanRasterRect {
            left: 0,
            top: 0,
            right: 32,
            bottom: 33
        }),
        Err(RetainedPaintError::IncompleteOverscanCoverage)
    ));
}

#[test]
fn source_buffer_blend_keeps_every_non_power_of_two_row() {
    let mut source = Canvas::new(29, 29);
    for y in 0..29usize {
        for x in 0..29usize {
            let index = (y * 29 + x) * 4;
            source.pixels[index..index + 4].copy_from_slice(&[x as u8, y as u8, 91, 255]);
        }
    }
    let mut output = Canvas::new(32, 32);
    blend_source_buffer(
        &mut output,
        &source,
        PixelRect {
            x: 2,
            y: 1,
            width: 29,
            height: 29,
        },
    );
    for y in 0..29usize {
        for x in 0..29usize {
            let index = ((y + 1) * 32 + x + 2) * 4;
            assert_eq!(
                &output.pixels[index..index + 4],
                &[x as u8, y as u8, 91, 255]
            );
        }
    }
    // Unit-scale image drawing must preserve the same exact pixel transfer.
    let mut sampled = Canvas::new(32, 32);
    sampled.draw_image(&source.into_image(), 2.0, 1.0);
    assert_eq!(output.pixels(), sampled.pixels());
}

#[test]
fn source_runtime_rejects_owner_clip_and_next_active_mismatches() {
    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 32.0,
        height: 32.0,
    };
    let make_recipe = |node, ordinal| {
        let geometry = geometry(
            Rect {
                x: 4.0,
                y: 4.0,
                width: 8.0,
                height: 8.0,
            },
            Default::default(),
            Default::default(),
        );
        FilterRecipe {
            node,
            bounds: geometry.at_scroll((0.0, 0.0)),
            geometry,
            clips: OwnedOverflowClipChain::default(),
            paint_owner_node_id: Some(7),
            paint_ordinal: Some(ordinal),
            paint_offset: Some(PaintOffsetKey::of(PaintOffset::default())),
            atomic_inline: None,
            filters: vec![crate::css::FilterFunction::Blur(1.0)],
            opacity: 1.0,
            active: true,
        }
    };
    let first = make_recipe(1, 0);
    let second = make_recipe(2, 1);
    let mut recipes = HashMap::new();
    recipes.insert(first.node, first.clone());
    recipes.insert(second.node, second.clone());
    let sources = FilterSourcePlan {
        recipes,
        ordered_active: vec![first.node, second.node],
        skipped_by_clip_owner: HashMap::new(),
        raw_fixed: HashMap::new(),
        raw_viewport_fixed_count: 0,
        raw_sticky: HashSet::new(),
        viewport,
        scroll: (0, 0),
        source_bytes: 0,
        scratch_bytes: 0,
        metadata_bytes: 0,
    };
    assert!(sources.invocation_matches(&first, Some(7), None, PaintOffset::default(), None));
    assert!(!sources.invocation_matches(&first, Some(8), None, PaintOffset::default(), None));
    assert!(!sources.invocation_matches(
        &first,
        Some(7),
        Some(Rect {
            width: 16.0,
            height: 16.0,
            ..viewport
        }),
        PaintOffset::default(),
        None
    ));

    let collector = RefCell::new(RetainedPaintCollector::new(ExtractionPlan {
        width: 32,
        height: 32,
        surface_bytes: 32 * 32 * 4,
        expected_runs: 5,
        fixed_roots: HashMap::new(),
        filter_sources: Some(sources),
        document_layers: None,
        document_origin: (0, 0),
    }));
    record_recipe(&collector, second, None);
    assert!(matches!(
        collector.borrow().error,
        Some(RetainedPaintError::TraversalMismatch)
    ));
    assert!(collector.borrow().runs.is_empty());
}
