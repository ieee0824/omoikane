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
fn cached_alpha_row_spans_composite_matches_full_surface_scan() {
    let expected = paint_pair(None);
    let cache = FrameShadowMaskCache::new(true);
    let actual = paint_pair(Some(&cache));
    assert_eq!(actual.pixels(), expected.pixels());

    let (key, rgba_bytes) =
        cache_entry_for(border_at(8.25, 12.5), 6.0, 1.25, Some((1.0, 4.0, 7.0, 2.5)));
    let (canvas, spans) = cache.lookup(&key, rgba_bytes).unwrap();
    let spans = spans.expect("this bounded test mask has a usable row-span index");
    assert_eq!(spans.row_count(), canvas.height() as usize);
    assert!((0..spans.row_count()).all(|row| {
        spans
            .row_spans(row)
            .iter()
            .all(|span| span.start < span.end)
    }));
}

#[test]
fn alpha_row_spans_index_nonzero_runs_and_fall_back_when_too_many() {
    let mut canvas = Canvas::new(7, 1);
    let alpha = Color::rgba(255, 0, 0, 255);
    for column in 0..3 {
        canvas.fill_rect(
            Rect {
                x: column as f32 * 3.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            },
            alpha,
        );
    }
    let spans = canvas.alpha_row_spans(3).unwrap();
    assert_eq!(spans.row_count(), 1);
    assert_eq!(spans.row_spans(0).len(), 3);
    assert!(spans.row_spans(0).iter().all(|span| span.start < span.end));
    assert!(canvas.alpha_row_spans(2).is_none());
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
    for index in 0..MAX_FRAME_SHADOW_MASK_ENTRIES {
        let key = synthetic_key(index as u32);
        assert!(entries.lookup(&key, 4).is_none());
        entries.record_generation(ShadowMaskGeneration::Eligible);
        entries.retain(key, 4, Rc::new(Canvas::new(1, 1)));
    }
    assert!(entries.lookup(&synthetic_key(0), 4).is_some());
    let newest = synthetic_key(MAX_FRAME_SHADOW_MASK_ENTRIES as u32);
    assert!(entries.lookup(&newest, 4).is_none());
    entries.record_generation(ShadowMaskGeneration::Eligible);
    entries.retain(newest, 4, Rc::new(Canvas::new(1, 1)));
    let summary = entries.summary().unwrap();
    assert_eq!(summary.generated, MAX_FRAME_SHADOW_MASK_ENTRIES + 1);
    assert_eq!(summary.retained_entries, MAX_FRAME_SHADOW_MASK_ENTRIES);
    assert_eq!(
        summary.retained_rgba_bytes,
        MAX_FRAME_SHADOW_MASK_ENTRIES * 4
    );
    assert_eq!(summary.eligible, MAX_FRAME_SHADOW_MASK_ENTRIES + 2);
    assert_eq!(summary.bypassed_capacity, 1);
    assert!(entries.lookup(&synthetic_key(0), 4).is_some());
    assert!(entries.lookup(&synthetic_key(1), 4).is_some());
    assert!(entries.lookup(&newest, 4).is_none());

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

fn retain_synthetic_entry(cache: &FrameShadowMaskCache, index: u32, rgba_capacity: usize) {
    let key = synthetic_key(index);
    assert!(cache.lookup(&key, rgba_capacity).is_none());
    cache.record_generation(ShadowMaskGeneration::Eligible);
    cache.retain(key, rgba_capacity, Rc::new(Canvas::new(1, 1)));
}

#[test]
fn persistent_entry_and_byte_caps_preserve_earlier_raster_entries_without_eviction() {
    let entries = FrameShadowMaskCache::new(false);
    for raster in 0..2 {
        let token = entries.begin_raster_measurement(true);
        for offset in 0..(MAX_FRAME_SHADOW_MASK_ENTRIES / 2) {
            retain_synthetic_entry(
                &entries,
                (raster * (MAX_FRAME_SHADOW_MASK_ENTRIES / 2) + offset) as u32,
                4,
            );
        }
        let summary = entries.finish_raster_measurement(token).unwrap();
        assert_eq!(
            summary.retained_entries,
            (raster + 1) * (MAX_FRAME_SHADOW_MASK_ENTRIES / 2)
        );
        assert_eq!(summary.bypassed_capacity, 0);
    }

    let rejected_entry = synthetic_key(MAX_FRAME_SHADOW_MASK_ENTRIES as u32);
    let token = entries.begin_raster_measurement(true);
    assert!(entries.lookup(&synthetic_key(0), 4).is_some());
    retain_synthetic_entry(&entries, MAX_FRAME_SHADOW_MASK_ENTRIES as u32, 4);
    let summary = entries.finish_raster_measurement(token).unwrap();
    assert_eq!(summary.retained_entries, MAX_FRAME_SHADOW_MASK_ENTRIES);
    assert_eq!((summary.hits, summary.prior_raster_hits), (1, 1));
    assert_eq!(summary.same_raster_hits, 0);
    assert_eq!(summary.bypassed_capacity, 1);
    assert!(entries.lookup(&synthetic_key(0), 4).is_some());
    assert!(entries.lookup(&rejected_entry, 4).is_none());

    let bytes = FrameShadowMaskCache::new(false);
    let entry_bytes = MAX_FRAME_SHADOW_MASK_ENTRY_RGBA_BYTES;
    let entries_per_raster = MAX_FRAME_SHADOW_MASK_RGBA_BYTES / entry_bytes / 2;
    for raster in 0..2 {
        let token = bytes.begin_raster_measurement(true);
        for offset in 0..entries_per_raster {
            retain_synthetic_entry(
                &bytes,
                (raster * entries_per_raster + offset) as u32,
                entry_bytes,
            );
        }
        let summary = bytes.finish_raster_measurement(token).unwrap();
        assert_eq!(
            summary.retained_rgba_bytes,
            (raster + 1) * entries_per_raster * entry_bytes
        );
        assert_eq!(summary.bypassed_capacity, 0);
    }

    let rejected_byte_entry = synthetic_key((entries_per_raster * 2) as u32);
    let token = bytes.begin_raster_measurement(true);
    assert!(bytes.lookup(&synthetic_key(0), entry_bytes).is_some());
    retain_synthetic_entry(&bytes, (entries_per_raster * 2) as u32, entry_bytes);
    let summary = bytes.finish_raster_measurement(token).unwrap();
    assert_eq!(
        summary.retained_rgba_bytes,
        MAX_FRAME_SHADOW_MASK_RGBA_BYTES
    );
    assert_eq!((summary.hits, summary.prior_raster_hits), (1, 1));
    assert_eq!(summary.same_raster_hits, 0);
    assert_eq!(summary.bypassed_capacity, 1);
    assert!(bytes.lookup(&synthetic_key(0), entry_bytes).is_some());
    assert!(bytes.lookup(&rejected_byte_entry, entry_bytes).is_none());
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
fn persistent_cache_classifies_same_and_prior_raster_hits_exactly() {
    let expected = paint_pair(None);
    let cache = FrameShadowMaskCache::new(false);

    let first_token = cache.begin_raster_measurement(true);
    let first = paint_pair(Some(&cache));
    let first_summary = cache.finish_raster_measurement(first_token).unwrap();
    assert_eq!(first.pixels(), expected.pixels());
    assert_eq!(
        (
            first_summary.eligible,
            first_summary.generated,
            first_summary.hits,
            first_summary.same_raster_hits,
            first_summary.prior_raster_hits,
        ),
        (2, 1, 1, 1, 0)
    );

    let second_token = cache.begin_raster_measurement(true);
    let second = paint_pair(Some(&cache));
    let second_summary = cache.finish_raster_measurement(second_token).unwrap();
    assert_eq!(second.pixels(), expected.pixels());
    assert_eq!(
        (
            second_summary.eligible,
            second_summary.generated,
            second_summary.hits,
            second_summary.same_raster_hits,
            second_summary.prior_raster_hits,
        ),
        (2, 0, 2, 0, 2)
    );
    assert_eq!(second_summary.hits, 2);
    assert_eq!(
        second_summary.hits,
        second_summary.same_raster_hits + second_summary.prior_raster_hits
    );
}

#[test]
fn late_measurement_classifies_trace_off_entries_as_prior_raster_hits() {
    let cache = FrameShadowMaskCache::new(false);
    let expected = paint_pair(None);
    let unmeasured = paint_pair(Some(&cache));
    assert_eq!(unmeasured.pixels(), expected.pixels());
    assert_eq!(cache.summary(), None);

    let token = cache.begin_raster_measurement(true);
    let measured = paint_pair(Some(&cache));
    let summary = cache.finish_raster_measurement(token).unwrap();
    assert_eq!(measured.pixels(), expected.pixels());
    assert_eq!((summary.generated, summary.hits), (0, 2));
    assert_eq!(
        (summary.same_raster_hits, summary.prior_raster_hits),
        (0, 2)
    );
}

#[test]
fn stale_and_exhausted_measurement_tokens_cannot_take_newer_counters() {
    let cache = FrameShadowMaskCache::new(false);
    let stale = cache.begin_raster_measurement(true);
    let disabled = cache.begin_raster_measurement(false);
    assert_eq!(cache.finish_raster_measurement(stale), None);
    assert_eq!(cache.finish_raster_measurement(disabled), None);
    assert_eq!(cache.summary(), None);

    let stale = cache.begin_raster_measurement(true);
    let current = cache.begin_raster_measurement(true);
    paint_pair(Some(&cache));
    assert_eq!(cache.finish_raster_measurement(stale), None);
    assert!(cache.finish_raster_measurement(current).is_some());

    cache.exhaust_measurement_tokens();
    let disabled = cache.begin_raster_measurement(true);
    paint_pair(Some(&cache));
    assert_eq!(cache.finish_raster_measurement(disabled), None);
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
