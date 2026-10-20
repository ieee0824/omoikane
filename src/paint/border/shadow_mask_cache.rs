//! Bounded ownership, exact keys, and raster-scoped counters for shadow masks.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::layout::Rect;

use super::super::Canvas;

pub(super) const MAX_FRAME_SHADOW_MASK_ENTRIES: usize = 128;
pub(super) const MAX_FRAME_SHADOW_MASK_RGBA_BYTES: usize = 16 * 1024 * 1024;
pub(super) const MAX_FRAME_SHADOW_MASK_ENTRY_RGBA_BYTES: usize = 2 * 1024 * 1024;

/// Owned aggregate for one document raster over a bounded shadow-mask cache.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ShadowMaskCacheSummary {
    pub(crate) eligible: usize,
    pub(crate) generated: usize,
    pub(crate) hits: usize,
    pub(crate) same_raster_hits: usize,
    pub(crate) prior_raster_hits: usize,
    pub(crate) generation_rgba_bytes_avoided: usize,
    pub(crate) bypassed_unkeyable: usize,
    pub(crate) bypassed_oversize: usize,
    pub(crate) bypassed_capacity: usize,
    pub(crate) retained_entries: usize,
    pub(crate) retained_rgba_bytes: usize,
}

impl ShadowMaskCacheSummary {
    pub(crate) fn merged(mut self, other: Self) -> Self {
        self.eligible = self.eligible.saturating_add(other.eligible);
        self.generated = self.generated.saturating_add(other.generated);
        self.hits = self.hits.saturating_add(other.hits);
        self.same_raster_hits = self.same_raster_hits.saturating_add(other.same_raster_hits);
        self.prior_raster_hits = self
            .prior_raster_hits
            .saturating_add(other.prior_raster_hits);
        self.generation_rgba_bytes_avoided = self
            .generation_rgba_bytes_avoided
            .saturating_add(other.generation_rgba_bytes_avoided);
        self.bypassed_unkeyable = self
            .bypassed_unkeyable
            .saturating_add(other.bypassed_unkeyable);
        self.bypassed_oversize = self
            .bypassed_oversize
            .saturating_add(other.bypassed_oversize);
        self.bypassed_capacity = self
            .bypassed_capacity
            .saturating_add(other.bypassed_capacity);
        self.retained_entries = self.retained_entries.saturating_add(other.retained_entries);
        self.retained_rgba_bytes = self
            .retained_rgba_bytes
            .saturating_add(other.retained_rgba_bytes);
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct RectBits {
    pub(super) x: u32,
    pub(super) y: u32,
    pub(super) width: u32,
    pub(super) height: u32,
}

impl RectBits {
    pub(super) fn from_finite(rect: Rect) -> Option<Self> {
        [rect.x, rect.y, rect.width, rect.height]
            .iter()
            .all(|value| value.is_finite())
            .then_some(Self {
                x: rect.x.to_bits(),
                y: rect.y.to_bits(),
                width: rect.width.to_bits(),
                height: rect.height.to_bits(),
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum ShadowMaskShapeKey {
    Rect,
    Rounded {
        source_radii: [u32; 4],
        knockout_radii: [u32; 4],
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum ShadowBlurPlanKey {
    RoundedZeroBlur,
    DirectGaussian { sigma_bits: u32 },
    Box { ordered_radii: [u32; 3] },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct ShadowMaskKey {
    pub(super) policy_version: u8,
    pub(super) allocated_width: u32,
    pub(super) allocated_height: u32,
    pub(super) local_rect: RectBits,
    pub(super) border_local: RectBits,
    pub(super) spread_bits: u32,
    pub(super) blur_radius_bits: u32,
    pub(super) shape: ShadowMaskShapeKey,
    pub(super) blur: ShadowBlurPlanKey,
}

#[derive(Default)]
struct FrameShadowMaskCacheState {
    entries: HashMap<ShadowMaskKey, CachedShadowMask>,
    retained_rgba_bytes: usize,
    next_measurement_token: Option<u64>,
    measurement: Option<RasterMeasurement>,
}

struct CachedShadowMask {
    canvas: Rc<Canvas>,
}

struct RasterMeasurement {
    token: u64,
    summary: ShadowMaskCacheSummary,
    inserted_keys: HashSet<ShadowMaskKey>,
}

/// Owned identity for one optional raster measurement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RasterMeasurementToken(Option<u64>);

#[derive(Clone, Copy)]
pub(super) enum ShadowMaskGeneration {
    Eligible,
    Unkeyable,
    Oversize,
}

/// Bounded shadow-mask cache whose owner determines its document lifetime.
pub(crate) struct FrameShadowMaskCache {
    state: RefCell<FrameShadowMaskCacheState>,
}

impl FrameShadowMaskCache {
    pub(crate) fn new(collect_metrics: bool) -> Self {
        let cache = Self {
            state: RefCell::new(FrameShadowMaskCacheState {
                next_measurement_token: Some(0),
                ..FrameShadowMaskCacheState::default()
            }),
        };
        let _ = cache.begin_raster_measurement(collect_metrics);
        cache
    }

    /// Starts fresh per-raster counters while preserving functional entries.
    pub(crate) fn begin_raster_measurement(&self, enabled: bool) -> RasterMeasurementToken {
        if !enabled {
            self.state.borrow_mut().measurement = None;
            return RasterMeasurementToken(None);
        }
        let mut state = self.state.borrow_mut();
        let Some(token) = state
            .next_measurement_token
            .and_then(|token| token.checked_add(1))
        else {
            state.next_measurement_token = None;
            state.measurement = None;
            return RasterMeasurementToken(None);
        };
        state.next_measurement_token = Some(token);
        state.measurement = Some(RasterMeasurement {
            token,
            summary: ShadowMaskCacheSummary::default(),
            inserted_keys: HashSet::new(),
        });
        RasterMeasurementToken(Some(token))
    }

    /// Finishes only the matching measurement and reports current occupancy.
    pub(crate) fn finish_raster_measurement(
        &self,
        token: RasterMeasurementToken,
    ) -> Option<ShadowMaskCacheSummary> {
        let token = token.0?;
        let mut state = self.state.borrow_mut();
        if state.measurement.as_ref().map(|sample| sample.token) != Some(token) {
            return None;
        }
        let mut summary = state.measurement.take()?.summary;
        summary.retained_entries = state.entries.len();
        summary.retained_rgba_bytes = state.retained_rgba_bytes;
        Some(summary)
    }

    /// Drops all functional entries and any unfinished measurement.
    pub(crate) fn clear(&self) {
        let mut state = self.state.borrow_mut();
        state.entries.clear();
        state.retained_rgba_bytes = 0;
        state.measurement = None;
    }

    pub(super) fn lookup(&self, key: &ShadowMaskKey, rgba_bytes: usize) -> Option<Rc<Canvas>> {
        let mut state = self.state.borrow_mut();
        let mask = state.entries.get(key).map(|entry| Rc::clone(&entry.canvas));
        let same_raster = mask.is_some()
            && state
                .measurement
                .as_ref()
                .is_some_and(|sample| sample.inserted_keys.contains(key));
        if let Some(measurement) = &mut state.measurement {
            let metrics = &mut measurement.summary;
            metrics.eligible = metrics.eligible.saturating_add(1);
            if mask.is_some() {
                metrics.hits = metrics.hits.saturating_add(1);
                if same_raster {
                    metrics.same_raster_hits = metrics.same_raster_hits.saturating_add(1);
                } else {
                    metrics.prior_raster_hits = metrics.prior_raster_hits.saturating_add(1);
                }
                metrics.generation_rgba_bytes_avoided = metrics
                    .generation_rgba_bytes_avoided
                    .saturating_add(rgba_bytes);
            }
        }
        mask
    }

    pub(super) fn record_generation(&self, kind: ShadowMaskGeneration) {
        let mut state = self.state.borrow_mut();
        let Some(measurement) = &mut state.measurement else {
            return;
        };
        let metrics = &mut measurement.summary;
        metrics.generated = metrics.generated.saturating_add(1);
        match kind {
            ShadowMaskGeneration::Eligible => {}
            ShadowMaskGeneration::Unkeyable => {
                metrics.bypassed_unkeyable = metrics.bypassed_unkeyable.saturating_add(1);
            }
            ShadowMaskGeneration::Oversize => {
                metrics.bypassed_oversize = metrics.bypassed_oversize.saturating_add(1);
            }
        }
    }

    pub(super) fn retain(&self, key: ShadowMaskKey, rgba_capacity: usize, mask: Rc<Canvas>) {
        let mut state = self.state.borrow_mut();
        if state.entries.contains_key(&key) {
            return;
        }
        if rgba_capacity > MAX_FRAME_SHADOW_MASK_ENTRY_RGBA_BYTES {
            if let Some(metrics) = state
                .measurement
                .as_mut()
                .map(|measurement| &mut measurement.summary)
            {
                metrics.bypassed_capacity = metrics.bypassed_capacity.saturating_add(1);
            }
            return;
        }
        let Some(next_bytes) = state.retained_rgba_bytes.checked_add(rgba_capacity) else {
            if let Some(metrics) = state
                .measurement
                .as_mut()
                .map(|measurement| &mut measurement.summary)
            {
                metrics.bypassed_capacity = metrics.bypassed_capacity.saturating_add(1);
            }
            return;
        };
        if state.entries.len() >= MAX_FRAME_SHADOW_MASK_ENTRIES
            || next_bytes > MAX_FRAME_SHADOW_MASK_RGBA_BYTES
        {
            if let Some(metrics) = state
                .measurement
                .as_mut()
                .map(|measurement| &mut measurement.summary)
            {
                metrics.bypassed_capacity = metrics.bypassed_capacity.saturating_add(1);
            }
            return;
        }
        if state
            .entries
            .insert(key, CachedShadowMask { canvas: mask })
            .is_none()
        {
            state.retained_rgba_bytes = next_bytes;
            if let Some(measurement) = &mut state.measurement {
                measurement.inserted_keys.insert(key);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn summary(&self) -> Option<ShadowMaskCacheSummary> {
        let state = self.state.borrow();
        state
            .measurement
            .as_ref()
            .map(|measurement| ShadowMaskCacheSummary {
                retained_entries: state.entries.len(),
                retained_rgba_bytes: state.retained_rgba_bytes,
                ..measurement.summary
            })
    }

    #[cfg(test)]
    pub(crate) fn retained_entries(&self) -> usize {
        self.state.borrow().entries.len()
    }

    #[cfg(test)]
    pub(crate) fn exhaust_measurement_tokens(&self) {
        let mut state = self.state.borrow_mut();
        state.next_measurement_token = None;
        state.measurement = None;
    }
}
