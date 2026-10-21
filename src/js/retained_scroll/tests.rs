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
    runtime.set_retained_scroll_capture(true);
    runtime.paint_current_document().unwrap();
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
