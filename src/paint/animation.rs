//! Owned image playback clocks and the regions observed by raster composition.

use super::{Canvas, Image, ImageAnimation, Rect, intersect};
use crate::css::AffineTransform;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub(crate) type ImageTimeline = Arc<Mutex<HashMap<(usize, String), u64>>>;

/// Selects a frame from a nonempty normalized (nonzero) delay sequence.
/// `duration_ms` is its sum; `plays` selects finite or infinite looping.
pub(super) fn animation_frame_index(
    delays_ms: &[u64],
    duration_ms: u64,
    plays: Option<u64>,
    elapsed_ms: u64,
) -> usize {
    if plays.is_some_and(|plays| elapsed_ms >= duration_ms.saturating_mul(plays)) {
        return delays_ms.len() - 1;
    }
    let mut position = elapsed_ms % duration_ms.max(1);
    for (index, delay) in delays_ms.iter().enumerate() {
        if position < *delay {
            return index;
        }
        position = position.saturating_sub(*delay);
    }
    delays_ms.len() - 1
}

/// Immutable timing data; demand queries only compare frame indices.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ImagePlayback {
    pub(crate) document: usize,
    source: String,
    start_ms: u64,
    delays_ms: Vec<u64>,
    duration_ms: u64,
    plays: Option<u64>,
}

impl ImagePlayback {
    pub(crate) fn frame_index(&self, now: u64) -> usize {
        animation_frame_index(
            &self.delays_ms,
            self.duration_ms,
            self.plays,
            now.saturating_sub(self.start_ms),
        )
    }

    pub(crate) fn running(&self, now: u64) -> bool {
        self.delays_ms.len() > 1
            && self.plays.is_none_or(|plays| {
                now.saturating_sub(self.start_ms) < self.duration_ms.saturating_mul(plays)
            })
    }
}

#[derive(Debug, Clone)]
pub(super) struct AnimationRegion {
    rect: Rect,
    playback: Arc<ImagePlayback>,
}

#[derive(Debug, Clone)]
pub(super) struct AnimatedPixels {
    animation: Arc<ImageAnimation>,
    playback: Arc<ImagePlayback>,
}

impl ImageAnimation {
    /// Adds an owned clock and keeps all frames available to cached layout.
    pub(crate) fn playback_image(
        self: &Arc<Self>,
        document: usize,
        source: &str,
        start_ms: u64,
        now: u64,
    ) -> Image {
        let playback = Arc::new(ImagePlayback {
            document,
            source: source.to_string(),
            start_ms,
            delays_ms: self.delays_ms.clone(),
            duration_ms: self.duration_ms,
            plays: self.plays,
        });
        let mut image = self.frames[playback.frame_index(now)].image.clone();
        image.animation_regions.push(AnimationRegion {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width: image.width as f32,
                height: image.height as f32,
            },
            playback: Arc::clone(&playback),
        });
        image.animated_pixels = Some(AnimatedPixels {
            animation: Arc::clone(self),
            playback,
        });
        image
    }
}

impl Image {
    pub(super) fn sampled_image(&self, now: u64) -> &Image {
        self.animated_pixels.as_ref().map_or(self, |animated| {
            &animated.animation.frames[animated.playback.frame_index(now)].image
        })
    }
}

impl Canvas {
    pub(crate) fn image_playbacks(&self) -> Vec<Arc<ImagePlayback>> {
        let mut result = Vec::new();
        for region in &self.animation_regions {
            if !result
                .iter()
                .any(|known: &Arc<ImagePlayback>| **known == *region.playback)
            {
                result.push(Arc::clone(&region.playback));
            }
        }
        result
    }

    pub(super) fn record_animation_regions_from_scaled_image(
        &mut self,
        image: &Image,
        destination: Rect,
        clip: Rect,
    ) {
        let transform = AffineTransform {
            a: destination.width / image.width.max(1) as f32,
            d: destination.height / image.height.max(1) as f32,
            e: destination.x,
            f: destination.y,
            ..AffineTransform::identity()
        };
        self.record_transformed_animation_regions(&image.animation_regions, transform, Some(clip));
    }

    pub(super) fn record_animation_regions_from_canvas(
        &mut self,
        source: &Canvas,
        transform: AffineTransform,
        clip: Option<Rect>,
    ) {
        self.record_transformed_animation_regions(&source.animation_regions, transform, clip);
    }

    fn record_transformed_animation_regions(
        &mut self,
        regions: &[AnimationRegion],
        transform: AffineTransform,
        clip: Option<Rect>,
    ) {
        let viewport = Rect {
            x: 0.0,
            y: 0.0,
            width: self.width as f32,
            height: self.height as f32,
        };
        for region in regions {
            let corners = [
                transform.transform_point(region.rect.x, region.rect.y),
                transform.transform_point(region.rect.x + region.rect.width, region.rect.y),
                transform.transform_point(region.rect.x, region.rect.y + region.rect.height),
                transform.transform_point(
                    region.rect.x + region.rect.width,
                    region.rect.y + region.rect.height,
                ),
            ];
            if corners
                .iter()
                .any(|(x, y)| !x.is_finite() || !y.is_finite())
            {
                continue;
            }
            let x = corners.iter().map(|p| p.0).fold(f32::INFINITY, f32::min);
            let y = corners.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
            let right = corners
                .iter()
                .map(|p| p.0)
                .fold(f32::NEG_INFINITY, f32::max);
            let bottom = corners
                .iter()
                .map(|p| p.1)
                .fold(f32::NEG_INFINITY, f32::max);
            let Some(mut rect) = intersect(
                Rect {
                    x,
                    y,
                    width: right - x,
                    height: bottom - y,
                },
                viewport,
            ) else {
                continue;
            };
            if let Some(clip) = clip {
                let Some(clipped) = intersect(rect, clip) else {
                    continue;
                };
                rect = clipped;
            }
            if rect.width > 0.0 && rect.height > 0.0 {
                self.animation_regions.push(AnimationRegion {
                    rect,
                    playback: Arc::clone(&region.playback),
                });
            }
        }
    }
}
