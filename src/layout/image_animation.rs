//! Traversal-scoped playback context; persistent clocks are owned by the runtime.

use super::{IMAGE_ANIMATION_TIME_MS, image_document_id};
use crate::paint::{Image, ImageAnimation, animation::ImageTimeline};
use std::cell::RefCell;
use std::sync::Arc;

thread_local! {
    static IMAGE_TIMELINE: RefCell<Option<ImageTimeline>> = const { RefCell::new(None) };
}

pub(crate) fn image_animation_time_ms() -> u64 {
    IMAGE_ANIMATION_TIME_MS.with(|time| time.get())
}

pub(crate) fn with_image_animation_timeline<T>(
    timeline: ImageTimeline,
    f: impl FnOnce() -> T,
) -> T {
    struct Restore(Option<ImageTimeline>);
    impl Drop for Restore {
        fn drop(&mut self) {
            IMAGE_TIMELINE.with(|cell| cell.replace(self.0.take()));
        }
    }
    let restore = Restore(IMAGE_TIMELINE.with(|cell| cell.replace(Some(timeline))));
    let result = f();
    drop(restore);
    result
}

pub(super) fn sample(animation: &Arc<ImageAnimation>, source: &str) -> Image {
    let now = image_animation_time_ms();
    let timeline = IMAGE_TIMELINE.with(|cell| cell.borrow().clone());
    let Some(timeline) = timeline else {
        return animation.frame_at(now).image().clone();
    };
    let document = image_document_id();
    let start = *timeline
        .lock()
        .unwrap()
        .entry((document, source.to_string()))
        .or_insert(now);
    animation.playback_image(document, source, start, now)
}
