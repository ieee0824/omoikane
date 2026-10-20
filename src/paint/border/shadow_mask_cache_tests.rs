use std::rc::Rc;

use super::*;

fn shadow(color: Color, blur_radius: f32, spread_radius: f32) -> BoxShadow {
    BoxShadow {
        offset_x: 2.25,
        offset_y: 1.5,
        blur_radius,
        spread_radius,
        color,
        inset: false,
    }
}

fn border_at(x: f32, y: f32) -> Rect {
    Rect {
        x,
        y,
        width: 24.0,
        height: 16.0,
    }
}

fn paint_pair_variant(
    cache: Option<&FrameShadowMaskCache>,
    blur_radius: f32,
    radii: Option<(f32, f32, f32, f32)>,
) -> Canvas {
    let mut canvas = Canvas::new(112, 56);
    canvas.fill_rect(
        Rect {
            x: 0.0,
            y: 0.0,
            width: 112.0,
            height: 56.0,
        },
        Color::rgba(240, 230, 220, 177),
    );
    let first = shadow(Color::rgba(210, 20, 30, 96), blur_radius, 1.25);
    let second = shadow(Color::rgba(20, 70, 220, 213), blur_radius, 1.25);
    let second_clip = Some(Rect {
        x: 55.5,
        y: 4.25,
        width: 47.0,
        height: 42.5,
    });
    if let Some(cache) = cache {
        paint_outer_box_shadow_cached(
            &mut canvas,
            border_at(8.25, 12.5),
            &first,
            None,
            radii,
            Some(cache),
        );
        paint_outer_box_shadow_cached(
            &mut canvas,
            border_at(62.25, 12.5),
            &second,
            second_clip,
            radii,
            Some(cache),
        );
    } else {
        paint_outer_box_shadow(&mut canvas, border_at(8.25, 12.5), &first, None, radii);
        paint_outer_box_shadow(
            &mut canvas,
            border_at(62.25, 12.5),
            &second,
            second_clip,
            radii,
        );
    }
    canvas
}

fn paint_pair(cache: Option<&FrameShadowMaskCache>) -> Canvas {
    paint_pair_variant(cache, 6.0, Some((1.0, 4.0, 7.0, 2.5)))
}

#[test]
fn reused_mask_preserves_color_alpha_clip_and_rgba_exactly() {
    let expected = paint_pair(None);
    let cache = FrameShadowMaskCache::new(true);
    let actual = paint_pair(Some(&cache));
    assert_eq!(actual.pixels(), expected.pixels());

    let summary = cache.summary().unwrap();
    assert_eq!(summary.eligible, 2);
    assert_eq!(summary.generated, 1);
    assert_eq!(summary.hits, 1);
    assert_eq!(summary.retained_entries, 1);
    assert_eq!(summary.bypassed_unkeyable, 0);
    assert_eq!(summary.bypassed_oversize, 0);
    assert_eq!(summary.bypassed_capacity, 0);
    assert_eq!(
        summary.generation_rgba_bytes_avoided,
        cache_entry_for(border_at(8.25, 12.5), 6.0, 1.25, Some((1.0, 4.0, 7.0, 2.5))).1
    );
    assert!(summary.retained_rgba_bytes > 0);
}

#[test]
fn zero_gaussian_and_box_blur_hits_match_uncached_rgba_exactly() {
    for (blur, radii) in [
        (0.0, Some((1.0, 4.0, 7.0, 2.5))),
        (3.0, None),
        (6.0, None),
        (6.0, Some((1.0, 4.0, 7.0, 2.5))),
    ] {
        let expected = paint_pair_variant(None, blur, radii);
        let cache = FrameShadowMaskCache::new(true);
        let actual = paint_pair_variant(Some(&cache), blur, radii);
        assert_eq!(
            actual.pixels(),
            expected.pixels(),
            "blur={blur} radii={radii:?}"
        );
        let summary = cache.summary().unwrap();
        assert_eq!(
            (summary.eligible, summary.generated, summary.hits),
            (2, 1, 1)
        );
    }
}

fn cache_entry_for(
    border: Rect,
    blur_radius: f32,
    spread_radius: f32,
    radii: Option<(f32, f32, f32, f32)>,
) -> (ShadowMaskKey, usize) {
    let shadow = shadow(Color::rgb(1, 2, 3), blur_radius, spread_radius);
    let shadow_rect = Rect {
        x: border.x + shadow.offset_x - spread_radius,
        y: border.y + shadow.offset_y - spread_radius,
        width: border.width + spread_radius * 2.0,
        height: border.height + spread_radius * 2.0,
    };
    shadow_mask_spec(border, shadow_rect, &shadow, radii)
        .cache_entry()
        .unwrap()
}

fn key_for(
    border: Rect,
    blur_radius: f32,
    spread_radius: f32,
    radii: Option<(f32, f32, f32, f32)>,
) -> ShadowMaskKey {
    cache_entry_for(border, blur_radius, spread_radius, radii).0
}

#[test]
fn key_is_translation_local_and_covers_fraction_spread_blur_and_some_zero_radii() {
    let base = border_at(8.25, 12.5);
    let base_key = key_for(base, 6.0, 0.0, None);
    assert_eq!(base_key, key_for(border_at(48.25, 32.5), 6.0, 0.0, None));
    assert_ne!(base_key, key_for(border_at(8.5, 12.5), 6.0, 0.0, None));
    assert_ne!(
        base_key,
        key_for(
            Rect {
                width: 25.0,
                ..base
            },
            6.0,
            0.0,
            None
        )
    );
    assert_ne!(base_key, key_for(base, 6.0, -0.0, None));
    assert_ne!(
        base_key,
        key_for(base, f32::from_bits(6.0f32.to_bits() + 1), 0.0, None)
    );
    assert_ne!(
        base_key,
        key_for(base, 6.0, 0.0, Some((0.0, 0.0, 0.0, 0.0)))
    );
    assert_ne!(
        key_for(base, 6.0, 0.0, Some((1.0, 2.0, 3.0, 4.0))),
        key_for(base, 6.0, 0.0, Some((1.0, 2.0, 3.0, 4.25)))
    );
}

fn synthetic_key(index: u32) -> ShadowMaskKey {
    ShadowMaskKey {
        policy_version: 1,
        allocated_width: index + 1,
        allocated_height: 1,
        local_rect: RectBits {
            x: index,
            y: 0,
            width: 0,
            height: 0,
        },
        border_local: RectBits {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        },
        spread_bits: 0,
        blur_radius_bits: 0,
        shape: ShadowMaskShapeKey::Rect,
        blur: ShadowBlurPlanKey::RoundedZeroBlur,
    }
}

#[test]
fn cache_enforces_entry_single_mask_and_total_byte_limits_without_eviction() {
    let entries = FrameShadowMaskCache::new(true);
    for index in 0..=MAX_FRAME_SHADOW_MASK_ENTRIES {
        let key = synthetic_key(index as u32);
        assert!(entries.lookup(&key, 4).is_none());
        entries.record_generation(ShadowMaskGeneration::Eligible);
        entries.retain(key, 4, Rc::new(Canvas::new(1, 1)));
    }
    let summary = entries.summary().unwrap();
    assert_eq!(summary.generated, MAX_FRAME_SHADOW_MASK_ENTRIES + 1);
    assert_eq!(summary.retained_entries, MAX_FRAME_SHADOW_MASK_ENTRIES);
    assert_eq!(
        summary.retained_rgba_bytes,
        MAX_FRAME_SHADOW_MASK_ENTRIES * 4
    );
    assert_eq!(summary.eligible, MAX_FRAME_SHADOW_MASK_ENTRIES + 1);
    assert_eq!(summary.bypassed_capacity, 1);

    let (_, oversized_bytes) = cache_entry_for(
        Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 700.0,
        },
        0.0,
        0.0,
        Some((0.0, 0.0, 0.0, 0.0)),
    );
    assert!(oversized_bytes > MAX_FRAME_SHADOW_MASK_ENTRY_RGBA_BYTES);
    let oversized = FrameShadowMaskCache::new(true);
    oversized.record_generation(ShadowMaskGeneration::Oversize);
    oversized.record_generation(ShadowMaskGeneration::Unkeyable);
    assert_eq!(
        oversized.summary().unwrap(),
        ShadowMaskCacheSummary {
            generated: 2,
            bypassed_unkeyable: 1,
            bypassed_oversize: 1,
            ..ShadowMaskCacheSummary::default()
        }
    );

    let bytes = FrameShadowMaskCache::new(true);
    for index in 0..9 {
        let key = synthetic_key(index);
        assert!(
            bytes
                .lookup(&key, MAX_FRAME_SHADOW_MASK_ENTRY_RGBA_BYTES)
                .is_none()
        );
        bytes.record_generation(ShadowMaskGeneration::Eligible);
        bytes.retain(
            key,
            MAX_FRAME_SHADOW_MASK_ENTRY_RGBA_BYTES,
            Rc::new(Canvas::new(1, 1)),
        );
    }
    let summary = bytes.summary().unwrap();
    assert_eq!(summary.generated, 9);
    assert_eq!(summary.retained_entries, 8);
    assert_eq!(
        summary.retained_rgba_bytes,
        MAX_FRAME_SHADOW_MASK_RGBA_BYTES
    );
    assert_eq!(summary.eligible, 9);
    assert_eq!(summary.bypassed_capacity, 1);
}

#[test]
fn nonfinite_raster_input_has_no_cache_key() {
    let shadow = shadow(Color::rgb(1, 2, 3), 6.0, f32::NAN);
    let border = border_at(8.25, 12.5);
    let spec = shadow_mask_spec(
        border,
        Rect {
            x: f32::NAN,
            y: border.y,
            width: border.width,
            height: border.height,
        },
        &shadow,
        Some((1.0, 1.0, 1.0, 1.0)),
    );
    assert!(spec.cache_entry().is_none());
}

#[test]
fn separate_document_raster_owners_do_not_share_masks() {
    let first = FrameShadowMaskCache::new(true);
    let second = FrameShadowMaskCache::new(true);
    let border = border_at(8.25, 12.5);
    let shadow = shadow(Color::rgb(10, 20, 30), 6.0, 1.25);
    for cache in [&first, &second] {
        paint_outer_box_shadow_cached(
            &mut Canvas::new(64, 48),
            border,
            &shadow,
            None,
            Some((2.0, 2.0, 2.0, 2.0)),
            Some(cache),
        );
    }
    for cache in [&first, &second] {
        assert_eq!(
            cache.summary().unwrap(),
            ShadowMaskCacheSummary {
                eligible: 1,
                generated: 1,
                retained_entries: 1,
                retained_rgba_bytes: cache.summary().unwrap().retained_rgba_bytes,
                ..ShadowMaskCacheSummary::default()
            }
        );
    }
}

#[test]
fn counted_document_raster_reports_reuse_from_its_own_owner() {
    let document = crate::html::TreeBuilder::parse(
        "<html><body><div class=a></div><div class=b></div></body></html>",
    )
    .document();
    let mut resolver = crate::css::StyleResolver::new();
    resolver.add_stylesheet(
        crate::css::Origin::Author,
        crate::css::parse_stylesheet(
            "html,body{margin:0}.a,.b{position:absolute;width:20px;height:12px;\
             border-radius:3px;box-shadow:2px 1px 6px 1px #2468ac}.a{left:10.25px}\
             .b{left:50.25px}",
        )
        .unwrap(),
    );
    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: 96.0,
        height: 48.0,
    };
    let root = document.query_selector("html").unwrap();
    let layout = crate::layout::layout_tree(&root, &mut resolver, viewport).unwrap();
    let (canvas, summary) = super::super::paint_layout_with_document_snapshots_counted(
        &layout,
        &mut resolver,
        viewport,
        Vec::new(),
        None,
        std::iter::empty(),
        &std::collections::HashMap::new(),
    );
    assert!(canvas.pixels().chunks_exact(4).any(|pixel| pixel[3] > 0));
    assert_eq!(summary.shadow_masks.eligible, 2);
    assert_eq!(summary.shadow_masks.generated, 1);
    assert_eq!(summary.shadow_masks.hits, 1);
    assert_eq!(summary.shadow_masks.retained_entries, 1);
}

#[test]
fn metrics_disabled_keeps_functional_reuse_without_counter_state() {
    let cache = FrameShadowMaskCache::new(false);
    let expected = paint_pair(None);
    let actual = paint_pair(Some(&cache));
    assert_eq!(actual.pixels(), expected.pixels());
    assert_eq!(cache.summary(), None);
}

#[test]
fn fully_offcanvas_shadow_is_rejected_before_cache_lookup() {
    let cache = FrameShadowMaskCache::new(true);
    paint_outer_box_shadow_cached(
        &mut Canvas::new(64, 48),
        border_at(10_000_000.0, 10_000_000.0),
        &shadow(Color::rgb(10, 20, 30), 8.0, 1.0),
        None,
        Some((2.0, 2.0, 2.0, 2.0)),
        Some(&cache),
    );
    assert_eq!(cache.summary().unwrap(), ShadowMaskCacheSummary::default());
}
