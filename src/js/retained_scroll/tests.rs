use super::*;
use crate::html::TreeBuilder;

fn runtime(extra_css: &str) -> JsRuntime {
    let html = format!(
        "<style>html,body{{margin:0}}body{{height:192px;background:white}}\
         .band{{height:32px}}#fixed{{position:fixed;top:2px;left:2px;width:5px;height:5px;background:blue}}\
         {extra_css}</style><div class=band style='background:red'></div>\
         <div class=band style='background:lime'></div><div class=band style='background:yellow'></div>\
         <div id=fixed></div>"
    );
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(&html).document()).unwrap();
    runtime.set_viewport(32.0, 32.0);
    runtime
}

#[test]
fn capture_configuration_and_interest_are_transition_idempotent() {
    let mut runtime = runtime("");
    let initial = runtime.render_generations().paint;

    runtime.set_retained_scroll_capture(true);
    let configured = runtime.render_generations().paint;
    assert_eq!(configured, initial + 1);
    runtime.set_retained_scroll_capture(true);
    assert_eq!(runtime.render_generations().paint, configured);
    runtime.paint_current_document().unwrap();
    runtime.set_retained_scroll_capture(true);
    assert!(runtime.take_retained_scroll_capture().is_some());

    runtime.set_retained_scroll_capture_interest(false);
    assert_eq!(runtime.render_generations().paint, configured);
    runtime.set_retained_scroll_capture_interest(false);
    assert_eq!(runtime.render_generations().paint, configured);
    runtime.paint_current_document().unwrap();
    assert!(runtime.take_retained_scroll_capture().is_none());

    runtime.set_retained_scroll_capture_interest(true);
    let interested = runtime.render_generations().paint;
    assert_eq!(interested, configured + 1);
    runtime.set_retained_scroll_capture_interest(true);
    assert_eq!(runtime.render_generations().paint, interested);
    runtime.paint_current_document().unwrap();
    runtime.set_retained_scroll_capture_interest(true);
    assert!(runtime.take_retained_scroll_capture().is_some());

    runtime.paint_current_document().unwrap();
    runtime.set_retained_scroll_capture_interest(false);
    assert!(runtime.take_retained_scroll_capture().is_none());
    let disabled = runtime.render_generations().paint;
    runtime.set_retained_scroll_capture(false);
    runtime.set_retained_scroll_capture(false);
    assert_eq!(runtime.render_generations().paint, disabled);
}

#[test]
fn runtime_capture_is_opt_in_and_preserves_the_current_frame() {
    let mut runtime = runtime("");
    let ordinary = runtime.paint_current_document().unwrap();
    assert!(runtime.take_retained_scroll_capture().is_none());
    runtime.set_retained_scroll_capture(true);
    let current = runtime.paint_current_document().unwrap();
    assert_eq!(current.pixels(), ordinary.pixels());
    let capture = runtime.take_retained_scroll_capture().unwrap();
    assert_eq!(capture.logical_scroll, (0.0, 0.0));
    assert!(capture.extent.1 >= 64.0);
    assert_eq!(
        capture.frame.compose_integer((0, 0)).unwrap().pixels(),
        current.pixels()
    );
    assert!(runtime.take_retained_scroll_capture().is_none());
    runtime.eval("window.scrollTo(0, 48)").unwrap();
    runtime.set_retained_scroll_capture(false);
    let later = runtime.paint_current_document().unwrap();
    assert_eq!(
        capture.frame.compose_integer((0, 48)).unwrap().pixels(),
        later.pixels()
    );
    assert!(runtime.take_retained_scroll_capture().is_none());
}

#[test]
fn fractional_logical_offset_is_kept_separate_from_integer_raster_samples() {
    let mut runtime = runtime("");
    runtime.eval("window.scrollTo(0, 47.5)").unwrap();
    let ordinary = runtime.paint_current_document().unwrap();
    runtime.set_retained_scroll_capture(true);
    let before = crate::paint::ordinary_document_raster_count();
    let current = runtime.paint_current_document().unwrap();
    assert_eq!(current.pixels(), ordinary.pixels());
    assert_eq!(crate::paint::ordinary_document_raster_count() - before, 1);
    let capture = runtime.take_retained_scroll_capture().unwrap();
    assert_eq!(capture.logical_scroll, (0.0, 47.5));
    assert_eq!(capture.frame.current_scroll(), (0, 32));
    assert_eq!(runtime.host_state.borrow().window_scroll, (0.0, 47.5));
    assert_eq!(capture.frame.coverage(), ((0, 0), (0, 64)));
}

#[test]
fn unsupported_capture_does_not_replace_the_ordinary_frame() {
    let mut runtime = runtime("#fixed{position:sticky}");
    let ordinary = runtime.paint_current_document().unwrap();
    runtime.set_retained_scroll_capture(true);
    let actual = runtime.paint_current_document().unwrap();
    assert_eq!(actual.pixels(), ordinary.pixels());
    assert!(runtime.take_retained_scroll_capture().is_none());
    assert_eq!(
        runtime.retained_scroll.reason,
        Some(RetainedPaintError::StickyPosition)
    );
}

#[test]
fn mutations_resize_and_scripted_scroll_discard_unconsumed_captures() {
    for mutation in [
        "document.body.style.background = 'black'",
        "window.scrollTo(0, 16)",
        "document.open();document.write('<p>new document</p>');document.close()",
    ] {
        let mut runtime = runtime("");
        runtime.set_retained_scroll_capture(true);
        runtime.paint_current_document().unwrap();
        runtime.eval(mutation).unwrap();
        assert!(
            runtime.take_retained_scroll_capture().is_none(),
            "{mutation}"
        );
    }
    let mut runtime = runtime("");
    runtime.set_retained_scroll_capture(true);
    runtime.paint_current_document().unwrap();
    runtime.set_viewport(48.0, 32.0);
    assert!(runtime.take_retained_scroll_capture().is_none());
}

#[test]
fn captured_pixels_outlive_the_runtime_and_are_sendable() {
    fn assert_send<T: Send>() {}
    assert_send::<CapturedScrollScene>();
    let scene = {
        let mut runtime = runtime("");
        runtime.set_retained_scroll_capture(true);
        runtime.paint_current_document().unwrap();
        runtime.take_retained_scroll_capture().unwrap()
    };
    assert_eq!(
        scene.frame.compose_integer((0, 32)).unwrap().pixel(3, 3),
        Some(crate::paint::Color::rgb(0, 0, 255))
    );
}

#[test]
fn current_retained_display_preserves_alpha_layers_at_integer_scrolls() {
    for css in ["", "#fixed{background:rgba(0,0,255,.5);opacity:.7}"] {
        for scroll in [0, 16, 32, 48] {
            let mut runtime = runtime(css);
            runtime
                .eval(&format!("window.scrollTo(0,{scroll})"))
                .unwrap();
            let ordinary = runtime.paint_current_document().unwrap();
            runtime.set_retained_scroll_capture(true);
            let before = crate::paint::ordinary_document_raster_count();
            let current = runtime.paint_current_document().unwrap();
            assert_eq!(
                current.pixels(),
                ordinary.pixels(),
                "scroll={scroll},css={css}"
            );
            assert_eq!(
                crate::paint::ordinary_document_raster_count() - before,
                usize::from(scroll % 32 != 0),
                "capture route scroll={scroll},css={css}"
            );
            assert!(runtime.take_retained_scroll_capture().is_some());
        }
    }
}

fn animated_data_uri(first: [u8; 4], second: [u8; 4]) -> String {
    use base64::Engine;
    let mut bytes = Vec::new();
    {
        let mut encoder = gif::Encoder::new(&mut bytes, 1, 1, &[]).unwrap();
        encoder.set_repeat(gif::Repeat::Infinite).unwrap();
        for (rgba, delay) in [(first, 1), (second, 50)] {
            let mut pixels = rgba.to_vec();
            let mut frame = gif::Frame::from_rgba_speed(1, 1, &mut pixels, 10);
            frame.delay = delay;
            encoder.write_frame(&frame).unwrap();
        }
    }
    format!(
        "data:image/gif;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

#[test]
fn current_retained_display_preserves_visible_animation_playbacks_and_clock() {
    let visible = animated_data_uri([255, 0, 0, 255], [0, 255, 0, 255]);
    let below = animated_data_uri([0, 0, 255, 255], [255, 255, 0, 255]);
    for include_below in [false, true] {
        let below_html = if include_below {
            format!("<img id=below src='{below}'>")
        } else {
            String::new()
        };
        let html = format!(
            "<style>html,body{{margin:0}}body{{height:192px;background:white}}\
             #visible{{position:fixed;top:0;left:0;width:2px;height:2px}}\
             #below{{position:absolute;top:60px;left:0;width:2px;height:2px}}</style>\
             <img id=visible src='{visible}'>{below_html}"
        );
        let mut runtime = JsRuntime::with_document(TreeBuilder::parse(&html).document()).unwrap();
        runtime.set_viewport(32.0, 32.0);
        runtime.run_until_idle().unwrap();
        let mut clock_pixels = Vec::new();
        for delta in [0, 20] {
            runtime.advance_clock_without_tasks(delta);
            runtime.set_retained_scroll_capture(false);
            let ordinary = runtime.paint_current_document().unwrap();
            let expected = ordinary.image_playbacks();
            assert_eq!(expected.len(), 1, "only the fixed image is visible");
            runtime.set_retained_scroll_capture(true);
            let current = runtime.paint_current_document().unwrap();
            let actual = current.image_playbacks();
            assert_eq!(current.pixels(), ordinary.pixels(), "below={include_below}");
            assert_eq!(actual.len(), expected.len());
            assert!(expected.iter().all(|playback| actual.contains(playback)));
            let scene = runtime.take_retained_scroll_capture();
            if include_below {
                // Visibility changes playback metadata across samples. Keep
                // this conservative traversal guard and the ordinary image.
                assert!(scene.is_none());
                assert_eq!(
                    runtime.retained_scroll.reason,
                    Some(RetainedPaintError::TraversalMismatch)
                );
            } else {
                assert!(scene.is_some(), "{:?}", runtime.retained_scroll.reason);
            }
            clock_pixels.push(current.pixel(0, 0));
        }
        assert_ne!(
            clock_pixels[0], clock_pixels[1],
            "animation must keep advancing"
        );
    }
}

#[test]
fn current_retained_display_matches_filters_sticky_clips_and_fallbacks() {
    for css in [
        "#fixed{filter:blur(2px);background:rgba(0,0,255,.5)}",
        "#fixed{position:sticky;top:0;background:rgba(0,0,255,.5)}",
        "body{overflow:hidden;border-radius:4px}#fixed{transform:rotate(5deg)}",
        "#fixed{backdrop-filter:blur(2px)}",
        "body{background-image:linear-gradient(red,blue);background-attachment:fixed}",
    ] {
        for scroll in [0, 48] {
            let mut runtime = runtime(css);
            runtime
                .eval(&format!("window.scrollTo(0,{scroll})"))
                .unwrap();
            let ordinary = runtime.paint_current_document().unwrap();
            runtime.set_retained_scroll_capture(true);
            let current = runtime.paint_current_document().unwrap();
            assert_eq!(
                current.pixels(),
                ordinary.pixels(),
                "scroll={scroll},css={css}"
            );
        }
    }
}

#[test]
fn rejected_capture_reuses_only_the_requested_display_origin() {
    for scroll in [0.0, 16.0, 32.0, 47.5] {
        let mut runtime = runtime("#fixed{backdrop-filter:blur(2px)}");
        runtime
            .eval(&format!("window.scrollTo(0,{scroll})"))
            .unwrap();
        let ordinary = runtime.paint_current_document().unwrap();
        runtime.set_retained_scroll_capture(true);
        let before = crate::paint::ordinary_document_raster_count();
        let current = runtime.paint_current_document().unwrap();
        assert_eq!(current.pixels(), ordinary.pixels(), "scroll={scroll}");
        assert!(runtime.take_retained_scroll_capture().is_none());
        assert_eq!(
            runtime.retained_scroll.reason,
            Some(RetainedPaintError::BackdropFilter)
        );
        assert_eq!(
            crate::paint::ordinary_document_raster_count() - before,
            if scroll == 0.0 || scroll == 32.0 {
                1
            } else {
                2
            },
            "fallback route scroll={scroll}"
        );
    }
}
