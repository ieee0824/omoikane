//! Bounded reuse of pre-opacity child-document effect rasters.

use super::Canvas;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

const MAX_CACHE_PIXELS: usize = 4 * 1024 * 1024;

/// Per-raster diagnostics for opacity source reuse and conservative fallbacks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct EffectSourceCacheSummary {
    pub(crate) hits: usize,
    pub(crate) source_rasters: usize,
    pub(crate) node_not_eligible: usize,
    pub(crate) descendant_animation: usize,
    pub(crate) inherited_clip: usize,
    pub(crate) noncanonical_phase: usize,
    pub(crate) transform: usize,
    pub(crate) other_effect: usize,
    pub(crate) nested_snapshot: usize,
    pub(crate) non_root_destination: usize,
    pub(crate) animated_image: usize,
    pub(crate) oversize: usize,
}

impl EffectSourceCacheSummary {
    pub(crate) fn merged(mut self, other: Self) -> Self {
        self.hits = self.hits.saturating_add(other.hits);
        self.source_rasters = self.source_rasters.saturating_add(other.source_rasters);
        self.node_not_eligible = self
            .node_not_eligible
            .saturating_add(other.node_not_eligible);
        self.descendant_animation = self
            .descendant_animation
            .saturating_add(other.descendant_animation);
        self.inherited_clip = self.inherited_clip.saturating_add(other.inherited_clip);
        self.noncanonical_phase = self
            .noncanonical_phase
            .saturating_add(other.noncanonical_phase);
        self.transform = self.transform.saturating_add(other.transform);
        self.other_effect = self.other_effect.saturating_add(other.other_effect);
        self.nested_snapshot = self.nested_snapshot.saturating_add(other.nested_snapshot);
        self.non_root_destination = self
            .non_root_destination
            .saturating_add(other.non_root_destination);
        self.animated_image = self.animated_image.saturating_add(other.animated_image);
        self.oversize = self.oversize.saturating_add(other.oversize);
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InsertOutcome {
    Stored,
    AnimatedImage,
    Oversize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct EffectSourceKey {
    pub(super) node: usize,
    pub(super) generation: u64,
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) inherited_clip: Option<[u32; 4]>,
    pub(super) include_phase_descendants: bool,
    pub(super) paint_decorations: bool,
    pub(super) offset: [u32; 2],
    pub(super) retained_paint_owner: Option<usize>,
    pub(super) retained_atomic_inline: Option<[usize; 4]>,
}

struct Entry {
    canvas: Canvas,
    last_used: u64,
}

/// Per-document cache. Interior mutability keeps its borrow local to lookup and
/// insertion rather than propagating a mutable borrow through paint traversal.
#[derive(Default)]
pub(crate) struct EffectSourceCache {
    entries: RefCell<HashMap<EffectSourceKey, Entry>>,
    clock: Cell<u64>,
    #[cfg(test)]
    source_rasters: Cell<u64>,
}

impl EffectSourceCache {
    pub(super) fn get(&self, key: EffectSourceKey) -> Option<Canvas> {
        let tick = self.next_tick();
        let mut entries = self.entries.borrow_mut();
        let entry = entries.get_mut(&key)?;
        entry.last_used = tick;
        Some(entry.canvas.clone())
    }

    pub(super) fn insert(&self, key: EffectSourceKey, canvas: Canvas) -> InsertOutcome {
        let pixels = canvas.width() as usize * canvas.height() as usize;
        if pixels == 0 || pixels > MAX_CACHE_PIXELS {
            return InsertOutcome::Oversize;
        }
        if !canvas.image_playbacks().is_empty() {
            return InsertOutcome::AnimatedImage;
        }
        let tick = self.next_tick();
        let mut entries = self.entries.borrow_mut();
        entries.retain(|candidate, _| candidate.node != key.node || *candidate == key);
        entries.insert(
            key,
            Entry {
                canvas,
                last_used: tick,
            },
        );
        while entries
            .values()
            .map(|entry| entry.canvas.width() as usize * entry.canvas.height() as usize)
            .sum::<usize>()
            > MAX_CACHE_PIXELS
        {
            let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| *key)
            else {
                break;
            };
            entries.remove(&oldest);
        }
        #[cfg(test)]
        self.source_rasters
            .set(self.source_rasters.get().saturating_add(1));
        InsertOutcome::Stored
    }

    fn next_tick(&self) -> u64 {
        let tick = self.clock.get().saturating_add(1);
        self.clock.set(tick);
        tick
    }

    #[cfg(test)]
    pub(crate) fn source_rasters(&self) -> u64 {
        self.source_rasters.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paint::{Image, Rect};
    use std::sync::Arc;

    #[test]
    fn animated_image_source_is_not_retained() {
        let mut bytes = Vec::new();
        {
            let mut encoder = gif::Encoder::new(&mut bytes, 1, 1, &[]).unwrap();
            encoder.set_repeat(gif::Repeat::Infinite).unwrap();
            for rgba in [[255, 0, 0, 255], [0, 255, 0, 255]] {
                let mut pixels = rgba.to_vec();
                let mut frame = gif::Frame::from_rgba_speed(1, 1, &mut pixels, 10);
                frame.delay = 1;
                encoder.write_frame(&frame).unwrap();
            }
        }
        let animation = Arc::new(Image::decode_gif_animation(&bytes).unwrap());
        let image = animation.playback_image(1, "fixture.gif", 0, 0);
        let mut canvas = Canvas::new(1, 1);
        canvas.draw_image_scaled_clipped(
            &image,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            },
            None,
        );
        let key = EffectSourceKey {
            node: 1,
            generation: 1,
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            inherited_clip: None,
            include_phase_descendants: true,
            paint_decorations: true,
            offset: [0; 2],
            retained_paint_owner: None,
            retained_atomic_inline: None,
        };
        let cache = EffectSourceCache::default();
        assert_eq!(cache.insert(key, canvas), InsertOutcome::AnimatedImage);
        assert!(cache.get(key).is_none());
        assert_eq!(cache.source_rasters(), 0);
    }

    #[test]
    fn pixel_budget_evicts_oldest_source_without_changing_retained_images() {
        let cache = EffectSourceCache::default();
        let key = |node| EffectSourceKey {
            node,
            generation: 1,
            x: 0,
            y: 0,
            width: 1024,
            height: 1024,
            inherited_clip: None,
            include_phase_descendants: true,
            paint_decorations: true,
            offset: [0; 2],
            retained_paint_owner: None,
            retained_atomic_inline: None,
        };
        for node in 1..=5 {
            let mut canvas = Canvas::new(1024, 1024);
            canvas.pixels[0] = node as u8;
            assert_eq!(cache.insert(key(node), canvas), InsertOutcome::Stored);
        }
        assert!(cache.get(key(1)).is_none());
        for node in 2..=5 {
            let retained = cache.get(key(node)).expect("newest four sources retained");
            assert_eq!(retained.pixels[0], node as u8);
        }
        let retained_pixels = cache
            .entries
            .borrow()
            .values()
            .map(|entry| entry.canvas.width() as usize * entry.canvas.height() as usize)
            .sum::<usize>();
        assert!(retained_pixels <= MAX_CACHE_PIXELS);
    }
}
