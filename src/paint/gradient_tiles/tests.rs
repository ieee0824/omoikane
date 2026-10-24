use super::*;
use crate::paint::color::parse_gradient;

fn cache_bytes() -> usize {
    CACHE.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|cache| cache.entry.as_ref())
            .map_or(0, Entry::bytes)
    })
}

fn painted(gradient: &color::Gradient, repeat: (bool, bool), offset: (f32, f32)) -> Canvas {
    let mut canvas = Canvas::new(31, 29);
    crate::paint::paint_tiled_gradient_background_auto(
        &mut canvas,
        Rect {
            x: offset.0,
            y: offset.1,
            width: 17.25,
            height: 35.5,
        },
        Some(Rect {
            x: 1.25,
            y: 0.75,
            width: 26.5,
            height: 24.25,
        }),
        gradient,
        repeat.0,
        repeat.1,
    );
    canvas
}

#[test]
fn sharing_keeps_exact_pixels_for_fractional_repeat_and_all_gradient_kinds() {
    for css in [
        "linear-gradient(37deg,rgba(255,0,0,.3),blue)",
        "radial-gradient(ellipse at 25% 70%,red,transparent,blue)",
        "conic-gradient(from 40deg at 40% 60%,red,blue,red)",
        "repeating-linear-gradient(25deg,red 0px,blue 7px)",
    ] {
        let gradient = parse_gradient(css).unwrap();
        for repeat in [(true, true), (true, false), (false, true), (false, false)] {
            for offset in [(0.0, 0.0), (-3.25, -13.75), (2.5, 4.25)] {
                let expected = painted(&gradient, repeat, offset);
                let before = generation_count();
                {
                    let _scope = gradient_tile_cache_scope();
                    for _ in 0..3 {
                        let actual = painted(&gradient, repeat, offset);
                        assert_eq!(
                            actual.pixels(),
                            expected.pixels(),
                            "{css} {repeat:?} {offset:?}"
                        );
                    }
                    assert_eq!(
                        generation_count() - before,
                        usize::from(repeat.0 || repeat.1)
                    );
                }
                assert_eq!(cache_bytes(), 0);
            }
        }
    }
}

#[test]
fn keys_include_gradient_float_geometry_and_integer_dimensions() {
    let red_blue = parse_gradient("linear-gradient(red,blue)").unwrap();
    let blue_red = parse_gradient("linear-gradient(blue,red)").unwrap();
    let before = generation_count();
    let _scope = gradient_tile_cache_scope();
    for (gradient, w, h, pw, ph) in [
        (&red_blue, 8.25, 9.5, 9, 10),
        (&red_blue, 8.25, 9.5, 9, 10),
        (&blue_red, 8.25, 9.5, 9, 10),
        (&blue_red, 8.5, 9.5, 9, 10),
        (&blue_red, 8.5, 9.25, 9, 10),
        (&blue_red, 8.5, 9.25, 10, 10),
    ] {
        drop(gradient_tile(gradient, w, h, pw, ph).unwrap());
    }
    assert_eq!(generation_count() - before, 5);
}

#[test]
fn frozen_miss_evicts_before_transient_raster_and_never_populates_cache() {
    let gradient = parse_gradient("linear-gradient(red,blue)").unwrap();
    let _scope = gradient_tile_cache_scope();
    let first = gradient_tile(&gradient, 16.0, 16.0, 16, 16).unwrap();
    let weak = Arc::downgrade(&first);
    drop(first);
    let _frozen = freeze_gradient_tiles();
    drop(gradient_tile(&gradient, 16.0, 16.0, 16, 16).unwrap());
    assert!(
        weak.upgrade().is_some(),
        "a frozen hit shares the ordinary raster"
    );
    drop(gradient_tile(&gradient, 17.0, 16.0, 17, 16).unwrap());
    assert!(
        weak.upgrade().is_none(),
        "a miss releases the old image allocation"
    );
    assert_eq!(cache_bytes(), 0);
}

#[test]
fn retained_budget_evicts_cache_without_relaxing_rejection_and_scope_unwinds() {
    let gradient = parse_gradient("linear-gradient(red,blue)").unwrap();
    let before = generation_count();
    for _ in 0..2 {
        let _scope = gradient_tile_cache_scope();
        drop(gradient_tile(&gradient, 16.0, 16.0, 16, 16).unwrap());
        let bytes = cache_bytes();
        let _frozen = freeze_gradient_tiles();
        assert!(retained_peak_fits(4000, 4000 + bytes));
        assert_eq!(cache_bytes(), bytes);
        assert!(retained_peak_fits(4001, 4000 + bytes));
        assert_eq!(cache_bytes(), 0);
        assert!(!retained_peak_fits(5001, 5000));
        assert!(gradient_tile(&gradient, 1.0, 1.0, u32::MAX, u32::MAX).is_none());
    }
    assert_eq!(generation_count() - before, 2);
    let _ = std::panic::catch_unwind(|| {
        let _scope = gradient_tile_cache_scope();
        drop(gradient_tile(&gradient, 16.0, 16.0, 16, 16).unwrap());
        panic!("scope unwind probe");
    });
    assert!(CACHE.with(|slot| slot.borrow().is_none()));
}

#[test]
fn nested_scope_unwind_preserves_outer_entry_and_unfreezes_it() {
    let gradient = parse_gradient("linear-gradient(red,blue)").unwrap();
    let before = generation_count();
    {
        let _outer = gradient_tile_cache_scope();
        let tile = gradient_tile(&gradient, 16.0, 16.0, 16, 16).unwrap();
        let weak = Arc::downgrade(&tile);
        drop(tile);
        let result = std::panic::catch_unwind(|| {
            let _nested = gradient_tile_cache_scope();
            let _frozen = freeze_gradient_tiles();
            drop(gradient_tile(&gradient, 16.0, 16.0, 16, 16).unwrap());
            panic!("nested render unwind probe");
        });
        assert!(result.is_err());
        assert!(
            weak.upgrade().is_some(),
            "nested unwind must retain the outer cache"
        );
        assert!(CACHE.with(|slot| {
            slot.borrow()
                .as_ref()
                .is_some_and(|cache| cache.frozen == 0)
        }));
        drop(gradient_tile(&gradient, 16.0, 16.0, 16, 16).unwrap());
        assert_eq!(generation_count() - before, 1);
        // Successful insertion after unwinding proves the inner frozen guard
        // did not remain latched on the outer rendering opportunity.
        drop(gradient_tile(&gradient, 17.0, 16.0, 17, 16).unwrap());
        assert_eq!(generation_count() - before, 2);
        assert!(cache_bytes() > 0);
        assert!(weak.upgrade().is_none());
    }
    assert!(CACHE.with(|slot| slot.borrow().is_none()));
}
