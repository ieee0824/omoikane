use super::*;
use crate::html::TreeBuilder;

#[test]
fn repeated_identical_canvas_pixels_preserve_document_paint_revision() {
    let mut runtime = runtime("");
    runtime.eval("globalThis.c=document.createElement('canvas'); c.width=2; c.height=2; document.body.appendChild(c); globalThis.ctx=c.getContext('2d'); ctx.fillStyle='red'; ctx.fillRect(0,0,2,2)").unwrap();
    let revision = || {
        let state = runtime.host_state.borrow();
        state.document_paint_revision(state.document.identity())
    };
    let before = revision();
    runtime.eval("ctx.fillRect(0,0,2,2)").unwrap();
    let unchanged = {
        let state = runtime.host_state.borrow();
        state.document_paint_revision(state.document.identity())
    };
    assert_eq!(before, unchanged);
    runtime
        .eval("ctx.fillStyle='blue'; ctx.fillRect(0,0,2,2)")
        .unwrap();
    let changed = {
        let state = runtime.host_state.borrow();
        state.document_paint_revision(state.document.identity())
    };
    assert!(changed.source > unchanged.source);
}

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
fn unchanged_source_republishes_shared_capture_after_take_and_scroll() {
    let mut runtime = runtime("");
    runtime.set_retained_scroll_capture(true);
    runtime.paint_current_document().unwrap();
    let first = runtime.take_retained_scroll_capture().unwrap();

    runtime.eval("window.scrollTo(0, 16)").unwrap();
    let before = crate::paint::ordinary_document_raster_count();
    let current = runtime.paint_current_document().unwrap();
    assert_eq!(crate::paint::ordinary_document_raster_count(), before);
    let second = runtime.take_retained_scroll_capture().unwrap();
    assert!(Arc::ptr_eq(&first.frame, &second.frame));
    assert_eq!(
        second.frame.compose_integer((0, 16)).unwrap().pixels(),
        current.pixels()
    );
}

#[test]
fn main_source_change_replaces_the_reusable_capture() {
    let mut runtime = runtime("");
    runtime.set_retained_scroll_capture(true);
    runtime.paint_current_document().unwrap();
    let first = runtime.take_retained_scroll_capture().unwrap();

    runtime
        .eval("document.body.style.background = 'black'")
        .unwrap();
    runtime.paint_current_document().unwrap();
    let second = runtime.take_retained_scroll_capture().unwrap();
    assert!(!Arc::ptr_eq(&first.frame, &second.frame));
}

#[test]
fn child_snapshot_match_requires_the_complete_owner_pixel_map() {
    let image = crate::paint::Image::new(1, 1, vec![1, 2, 3, 255]).unwrap();
    let changed = crate::paint::Image::new(1, 1, vec![1, 2, 4, 255]).unwrap();
    let left = HashMap::from([(7, image.clone())]);
    assert!(snapshots_exact(&left, &HashMap::from([(7, image)])));
    assert!(!snapshots_exact(&left, &HashMap::from([(7, changed)])));
    assert!(!snapshots_exact(
        &left,
        &HashMap::from([(8, left[&7].clone())])
    ));
    assert!(!snapshots_exact(&left, &HashMap::new()));
}

fn animated_snapshot() -> crate::paint::Image {
    let mut bytes = Vec::new();
    {
        let mut encoder = gif::Encoder::new(&mut bytes, 1, 1, &[]).unwrap();
        encoder.set_repeat(gif::Repeat::Infinite).unwrap();
        for color in [[255, 0, 0, 255], [0, 0, 255, 255]] {
            let mut pixels = color.to_vec();
            let mut frame = gif::Frame::from_rgba_speed(1, 1, &mut pixels, 10);
            frame.delay = 1;
            encoder.write_frame(&frame).unwrap();
        }
    }
    Arc::new(crate::paint::Image::decode_gif_animation(&bytes).unwrap()).playback_image(
        7,
        "test:animated-child",
        0,
        0,
    )
}

#[test]
fn child_snapshot_animation_metadata_fails_closed() {
    let image = animated_snapshot();
    assert!(!snapshots_exact(
        &HashMap::from([(7, image.clone())]),
        &HashMap::from([(7, image)])
    ));
}

fn animated_iframe_runtime(child_target_css: &str, main_css: &str) -> JsRuntime {
    let child = format!(
        "<style>html,body{{margin:0;background:lime}}#target{{{child_target_css}}}\
         @keyframes pulse{{from{{opacity:1}}to{{opacity:.2}}}}</style><div id=target></div>"
    );
    let html = format!(
        "<style>html,body{{margin:0}}body{{height:192px;background:white}}\
         iframe{{display:block;width:20px;height:20px;border:0}}{main_css}\
         @keyframes main-pulse{{from{{opacity:1}}to{{opacity:.2}}}}</style>\
         <iframe id=frame srcdoc={}></iframe><div id=main></div>",
        serde_json::to_string(&child).unwrap()
    );
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(&html).document()).unwrap();
    runtime.set_viewport(32.0, 32.0);
    runtime.enable_live_css_animations();
    // Materialize the lazily-created child document and its style/font state
    // before testing cross-frame capture reuse. That first materialization may
    // legitimately invalidate the main iframe box and is not a reusable frame.
    runtime.paint_current_document().unwrap();
    runtime
}

#[test]
fn unchanged_child_animation_pixels_republish_the_same_main_frame() {
    let mut runtime =
        animated_iframe_runtime("width:0;height:0;animation:pulse 1s linear infinite", "");
    runtime.set_retained_scroll_capture(true);
    runtime.paint_current_document().unwrap();
    let first = runtime.take_retained_scroll_capture().unwrap();
    let generation = runtime.render_generations().paint;
    runtime.advance_clock_without_tasks(100);
    runtime.paint_current_document().unwrap();
    assert!(runtime.render_generations().paint > generation);
    let second = runtime.take_retained_scroll_capture().unwrap();
    assert!(Arc::ptr_eq(&first.frame, &second.frame));
}

#[test]
fn changed_child_pixels_and_main_opacity_animation_replace_the_frame() {
    let mut child = animated_iframe_runtime("width:20px;height:20px", "");
    child.set_retained_scroll_capture(true);
    child.paint_current_document().unwrap();
    let first = child.take_retained_scroll_capture().unwrap();
    child
        .eval("document.getElementById('frame').contentDocument.body.style.background='blue'")
        .unwrap();
    let captured = child.paint_current_document().unwrap();
    let second = child.take_retained_scroll_capture().unwrap();
    assert!(!Arc::ptr_eq(&first.frame, &second.frame));
    child.set_retained_scroll_capture(false);
    assert_eq!(
        captured.pixels(),
        child.paint_current_document().unwrap().pixels()
    );

    let mut main = animated_iframe_runtime(
        "width:0;height:0",
        "#main{width:20px;height:20px;background:red;animation:main-pulse 1s linear infinite}",
    );
    main.set_retained_scroll_capture(true);
    main.paint_current_document().unwrap();
    let first = main.take_retained_scroll_capture().unwrap();
    main.advance_clock_without_tasks(100);
    main.paint_current_document().unwrap();
    let second = main.take_retained_scroll_capture().unwrap();
    assert!(!Arc::ptr_eq(&first.frame, &second.frame));
}

#[test]
fn coverage_and_disable_drop_reuse_state() {
    let mut runtime = runtime("");
    runtime.set_retained_scroll_capture(true);
    runtime.paint_current_document().unwrap();
    let first = runtime.take_retained_scroll_capture().unwrap();
    runtime.eval("window.scrollTo(0,96)").unwrap();
    runtime.paint_current_document().unwrap();
    let outside = runtime.take_retained_scroll_capture().unwrap();
    assert!(!Arc::ptr_eq(&first.frame, &outside.frame));
    runtime.set_retained_scroll_capture(false);
    assert!(runtime.retained_scroll.reusable.is_none());
}

#[test]
fn history_url_change_replaces_relative_input_capture() {
    let document = TreeBuilder::parse(
        "<style>html,body{margin:0}a{display:block;width:32px;height:192px;background:red}\
         a:visited{background:lime}</style><a href=target></a>",
    )
    .document();
    let storage = StorageManager::new();
    let session = storage.create_session();
    let source = VisitSource::new(
        StorageOrigin::from_url("https://example.test/a/page").unwrap(),
        "https://example.test/a/page",
    )
    .unwrap();
    storage.record_page_navigation(
        "https://example.test/a/target",
        "https://example.test/a/target",
        source,
    );
    let mut runtime = JsRuntime::with_document_url_and_storage(
        document,
        "https://example.test/a/page",
        storage,
        session,
    )
    .unwrap();
    runtime.set_viewport(32.0, 32.0);
    runtime.set_retained_scroll_capture(true);
    let visited = runtime.paint_current_document().unwrap();
    assert_eq!(
        visited.pixel(16, 16),
        Some(crate::paint::Color::rgb(0, 255, 0))
    );
    let first = runtime.take_retained_scroll_capture().unwrap();
    runtime.eval("history.pushState({},'', '/b/page')").unwrap();
    runtime.eval("window.scrollTo(0,16)").unwrap();
    let current = runtime.paint_current_document().unwrap();
    assert_eq!(
        current.pixel(16, 16),
        Some(crate::paint::Color::rgb(255, 0, 0))
    );
    let second = runtime.take_retained_scroll_capture().unwrap();
    assert!(!Arc::ptr_eq(&first.frame, &second.frame));

    runtime.set_retained_scroll_capture(false);
    let cold = runtime.paint_current_document().unwrap();
    assert_eq!(current.pixels(), cold.pixels());
}

#[test]
fn retired_child_document_drops_its_paint_revision() {
    let mut runtime = JsRuntime::with_document(
        TreeBuilder::parse("<iframe id=frame srcdoc='<p>old</p>'></iframe>").document(),
    )
    .unwrap();
    runtime
        .eval("void document.getElementById('frame').contentDocument.body.offsetWidth")
        .unwrap();
    let old = runtime
        .host_state
        .borrow()
        .iframe_documents
        .values()
        .next()
        .unwrap()
        .document
        .identity();
    runtime
        .host_state
        .borrow_mut()
        .invalidate_document_paint_source(old);
    assert!(
        runtime
            .host_state
            .borrow()
            .document_paint_revisions
            .contains_key(&old)
    );
    runtime
        .eval("document.getElementById('frame').srcdoc='<p>new</p>';void document.getElementById('frame').contentDocument")
        .unwrap();
    assert!(
        !runtime
            .host_state
            .borrow()
            .document_paint_revisions
            .contains_key(&old)
    );
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
