//! Bounded primitive-layer recording for retained paint runs.

use super::retained_extract::{RetainedPaintError, RetainedPaintRunIdentity};
use super::{AffineTransform, Canvas, Image, Rect};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub(super) const MAX_RETAINED_PRIMITIVES: usize = 1_024;
pub(super) const MAX_RETAINED_ANIMATION_REGIONS: usize = 1_024;

fn trace_layer_mismatch(stage: &str, detail: impl FnOnce() -> String) {
    if std::env::var_os("OMOIKANE_TRACE_RETAINED_LAYERS").is_some() {
        eprintln!(
            "OMOIKANE_RETAINED_LAYER_MISMATCH stage={stage} {}",
            detail()
        );
    }
}

pub(super) fn recorder_metadata_peak_bytes() -> Result<usize, RetainedPaintError> {
    let bounded = MAX_RETAINED_PRIMITIVES
        .checked_mul(std::mem::size_of::<PlannedPrimitive>())
        .and_then(|bytes| {
            MAX_RETAINED_PRIMITIVES
                .checked_mul(std::mem::size_of::<RetainedDocumentLayer>())
                .and_then(|value| bytes.checked_add(value))
        })
        .and_then(|bytes| {
            MAX_RETAINED_PRIMITIVES
                .checked_mul(std::mem::size_of::<PixelRect>())
                .and_then(|value| bytes.checked_add(value))
        });
    // The scratch metadata and its cropped copy coexist while a layer seals.
    bounded
        .and_then(|bytes| {
            MAX_RETAINED_ANIMATION_REGIONS
                .checked_mul(std::mem::size_of::<super::animation::AnimationRegion>())
                .and_then(|value| value.checked_mul(2))
                .and_then(|value| bytes.checked_add(value))
        })
        .ok_or(RetainedPaintError::Capacity)
}

pub(super) fn layer_plan_metadata_max_bytes(run_count: usize) -> Result<usize, RetainedPaintError> {
    let per_run = std::mem::size_of::<RetainedRunLayerPlan>()
        .checked_add(
            MAX_RETAINED_PRIMITIVES
                .checked_mul(std::mem::size_of::<PlannedPrimitive>())
                .ok_or(RetainedPaintError::Capacity)?,
        )
        .and_then(|bytes| {
            MAX_RETAINED_PRIMITIVES
                .checked_mul(std::mem::size_of::<PlannedLayer>())
                .and_then(|value| bytes.checked_add(value))
        })
        .ok_or(RetainedPaintError::Capacity)?;
    run_count
        .checked_mul(per_run)
        .ok_or(RetainedPaintError::Capacity)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RetainedPrimitiveKind {
    FillRect,
    RoundedRect,
    RoundedAnnulus,
    Image,
    Glyph,
    ShadowComposite,
    AffineComposite,
    EffectComposite,
    BackgroundComposite,
    Gradient,
    Triangle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RetainedPrimitiveToken {
    pub(super) ordinal: u32,
    pub(super) kind: RetainedPrimitiveKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PlannedPrimitive {
    pub(super) token: RetainedPrimitiveToken,
    pub(super) starts_layer: bool,
    write_rect: PixelRect,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RetainedRunLayerPlan {
    pub(super) identity: RetainedPaintRunIdentity,
    pub(super) base_first: bool,
    pub(super) primitives: Arc<[PlannedPrimitive]>,
    pub(super) layers: Arc<[PlannedLayer]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PlannedLayer {
    pub(super) identity: RetainedLayerIdentity,
    pub(super) sample_rect: PixelRect,
    pub(super) animation_count: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct RetainedLayerPlan {
    pub(super) runs: Vec<RetainedRunLayerPlan>,
}

impl RetainedLayerPlan {
    pub(super) fn run(&self, identity: RetainedPaintRunIdentity) -> Option<&RetainedRunLayerPlan> {
        self.runs.iter().find(|run| run.identity == identity)
    }

    pub(super) fn metadata_bytes(&self) -> Result<usize, RetainedPaintError> {
        let base = self
            .runs
            .capacity()
            .checked_mul(std::mem::size_of::<RetainedRunLayerPlan>())
            .ok_or(RetainedPaintError::Capacity)?;
        self.runs
            .iter()
            .try_fold(base, |sum, run| {
                let primitives = run
                    .primitives
                    .len()
                    .checked_mul(std::mem::size_of::<PlannedPrimitive>())?;
                let layers = run
                    .layers
                    .len()
                    .checked_mul(std::mem::size_of::<PlannedLayer>())?;
                sum.checked_add(primitives)?.checked_add(layers)
            })
            .ok_or(RetainedPaintError::Capacity)
    }
}

#[derive(Clone, Debug)]
pub(super) struct RetainedSparseImage {
    pub(super) origin: (i32, i32),
    pub(super) image: Image,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RetainedLayerIdentity {
    pub(super) first: RetainedPrimitiveToken,
    pub(super) last: RetainedPrimitiveToken,
    pub(super) operation_count: u32,
}

#[derive(Clone, Debug)]
pub(super) struct RetainedDocumentLayer {
    pub(super) identity: RetainedLayerIdentity,
    pub(super) sample_rect: PixelRect,
    pub(super) animation_count: usize,
    pub(super) image: Option<RetainedSparseImage>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PixelRect {
    pub(super) left: i32,
    pub(super) top: i32,
    pub(super) right: i32,
    pub(super) bottom: i32,
}

impl PixelRect {
    fn write_bounds(rect: Rect) -> Result<Self, RetainedPaintError> {
        let right = rect.x + rect.width;
        let bottom = rect.y + rect.height;
        if !rect.x.is_finite()
            || !rect.y.is_finite()
            || !rect.width.is_finite()
            || !rect.height.is_finite()
            || !right.is_finite()
            || !bottom.is_finite()
        {
            return Err(RetainedPaintError::TraversalMismatch);
        }
        let left = rect.x.floor() as f64;
        let top = rect.y.floor() as f64;
        if rect.width <= 0.0 || rect.height <= 0.0 {
            if [left, top]
                .into_iter()
                .any(|value| value < i32::MIN as f64 || value > i32::MAX as f64)
            {
                return Err(RetainedPaintError::TraversalMismatch);
            }
            return Ok(Self {
                left: left as i32,
                top: top as i32,
                right: left as i32,
                bottom: top as i32,
            });
        }
        let right = right.ceil() as f64;
        let bottom = bottom.ceil() as f64;
        if [left, top, right, bottom]
            .into_iter()
            .any(|value| value < i32::MIN as f64 || value > i32::MAX as f64)
        {
            return Err(RetainedPaintError::TraversalMismatch);
        }
        Ok(Self {
            left: left as i32,
            top: top as i32,
            right: right as i32,
            bottom: bottom as i32,
        })
    }

    fn sampling_support(self) -> Result<Self, RetainedPaintError> {
        if self.empty() {
            return Ok(self);
        }
        Ok(Self {
            left: self
                .left
                .checked_sub(1)
                .ok_or(RetainedPaintError::TraversalMismatch)?,
            top: self
                .top
                .checked_sub(1)
                .ok_or(RetainedPaintError::TraversalMismatch)?,
            right: self
                .right
                .checked_add(1)
                .ok_or(RetainedPaintError::TraversalMismatch)?,
            bottom: self
                .bottom
                .checked_add(1)
                .ok_or(RetainedPaintError::TraversalMismatch)?,
        })
    }

    fn clamp(self, width: u32, height: u32) -> Self {
        Self {
            left: self.left.clamp(0, width as i32),
            top: self.top.clamp(0, height as i32),
            right: self.right.clamp(0, width as i32),
            bottom: self.bottom.clamp(0, height as i32),
        }
    }

    pub(super) fn translated(self, offset: (i32, i32)) -> Result<Self, RetainedPaintError> {
        Ok(Self {
            left: self
                .left
                .checked_add(offset.0)
                .ok_or(RetainedPaintError::TraversalMismatch)?,
            top: self
                .top
                .checked_add(offset.1)
                .ok_or(RetainedPaintError::TraversalMismatch)?,
            right: self
                .right
                .checked_add(offset.0)
                .ok_or(RetainedPaintError::TraversalMismatch)?,
            bottom: self
                .bottom
                .checked_add(offset.1)
                .ok_or(RetainedPaintError::TraversalMismatch)?,
        })
    }

    fn empty(self) -> bool {
        self.left >= self.right || self.top >= self.bottom
    }

    fn intersects(self, other: Self) -> bool {
        !self.empty()
            && !other.empty()
            && self.left < other.right
            && other.left < self.right
            && self.top < other.bottom
            && other.top < self.bottom
    }

    fn union(self, other: Self) -> Self {
        if self.empty() {
            return other;
        }
        if other.empty() {
            return self;
        }
        Self {
            left: self.left.min(other.left),
            top: self.top.min(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }

    pub(super) fn intersection(self, other: Self) -> Option<Self> {
        let result = Self {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        };
        (!result.empty()).then_some(result)
    }

    pub(super) fn bytes(self) -> Option<usize> {
        (self.right - self.left)
            .max(0)
            .try_into()
            .ok()
            .and_then(|width: usize| {
                (self.bottom - self.top)
                    .max(0)
                    .try_into()
                    .ok()
                    .and_then(|height: usize| width.checked_mul(height))
            })
            .and_then(|pixels| pixels.checked_mul(4))
    }
}

fn copy_retained_region(source: &Canvas, bounds: PixelRect) -> Result<Canvas, RetainedPaintError> {
    let width =
        u32::try_from(bounds.right - bounds.left).map_err(|_| RetainedPaintError::Capacity)?;
    let height =
        u32::try_from(bounds.bottom - bounds.top).map_err(|_| RetainedPaintError::Capacity)?;
    let bytes = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(RetainedPaintError::Capacity)?;
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(bytes)
        .map_err(|_| RetainedPaintError::Capacity)?;
    pixels.resize(bytes, 0);
    let mut animation_regions = Vec::new();
    animation_regions
        .try_reserve_exact(source.animation_regions.len())
        .map_err(|_| RetainedPaintError::Capacity)?;
    let mut result = Canvas {
        width,
        height,
        pixels,
        animation_regions,
        ink_probe: None,
        retained_layers: None,
        retained_operation_guard: None,
    };
    result.record_animation_regions_from_canvas(
        source,
        AffineTransform::translate(-(bounds.left as f32), -(bounds.top as f32)),
        None,
    );
    let source_stride = source.width as usize * 4;
    let row_bytes = width as usize * 4;
    for row in 0..height as usize {
        let source_start = (bounds.top as usize + row) * source_stride + bounds.left as usize * 4;
        let destination_start = row * row_bytes;
        result.pixels[destination_start..destination_start + row_bytes]
            .copy_from_slice(&source.pixels[source_start..source_start + row_bytes]);
    }
    Ok(result)
}

#[derive(Debug)]
enum RecorderMode {
    Plan,
    FlatCapture,
    Capture {
        expected: Arc<[PlannedPrimitive]>,
        cursor: usize,
    },
}

#[derive(Debug)]
pub(super) struct CanvasLayerRecorder {
    mode: RecorderMode,
    scratch_pixels: Vec<u8>,
    scratch_animation_regions: Vec<super::animation::AnimationRegion>,
    primitives: Vec<PlannedPrimitive>,
    layers: Vec<RetainedDocumentLayer>,
    current_sample_rect: Option<PixelRect>,
    current_first: Option<RetainedPrimitiveToken>,
    current_last: Option<RetainedPrimitiveToken>,
    current_operations: u32,
    error: Option<RetainedPaintError>,
    invalidated: Arc<AtomicBool>,
    coordinate_origin: (i32, i32),
    current_support: Option<PixelRect>,
    current_writes: Vec<PixelRect>,
}

impl CanvasLayerRecorder {
    pub(super) fn planning(
        width: u32,
        height: u32,
        coordinate_origin: (i32, i32),
    ) -> Result<Self, RetainedPaintError> {
        Self::new(width, height, coordinate_origin, RecorderMode::Plan)
    }

    pub(super) fn capturing(
        width: u32,
        height: u32,
        coordinate_origin: (i32, i32),
        expected: Arc<[PlannedPrimitive]>,
    ) -> Result<Self, RetainedPaintError> {
        Self::new(
            width,
            height,
            coordinate_origin,
            RecorderMode::Capture {
                expected,
                cursor: 0,
            },
        )
    }

    pub(super) fn capturing_flat(
        width: u32,
        height: u32,
        coordinate_origin: (i32, i32),
    ) -> Result<Self, RetainedPaintError> {
        Self::new(width, height, coordinate_origin, RecorderMode::FlatCapture)
    }

    fn new(
        width: u32,
        height: u32,
        coordinate_origin: (i32, i32),
        mode: RecorderMode,
    ) -> Result<Self, RetainedPaintError> {
        if matches!(&mode, RecorderMode::Capture { expected, .. } if expected.len() > MAX_RETAINED_PRIMITIVES)
        {
            return Err(RetainedPaintError::Capacity);
        }
        let bytes = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or(RetainedPaintError::Capacity)?;
        let mut scratch_pixels = Vec::new();
        scratch_pixels
            .try_reserve_exact(bytes)
            .map_err(|_| RetainedPaintError::Capacity)?;
        scratch_pixels.resize(bytes, 0);
        let mut scratch_animation_regions = Vec::new();
        scratch_animation_regions
            .try_reserve_exact(MAX_RETAINED_ANIMATION_REGIONS)
            .map_err(|_| RetainedPaintError::Capacity)?;
        let mut primitives = Vec::new();
        primitives
            .try_reserve_exact(MAX_RETAINED_PRIMITIVES)
            .map_err(|_| RetainedPaintError::Capacity)?;
        let mut layers = Vec::new();
        layers
            .try_reserve_exact(MAX_RETAINED_PRIMITIVES)
            .map_err(|_| RetainedPaintError::Capacity)?;
        let mut current_writes = Vec::new();
        current_writes
            .try_reserve_exact(MAX_RETAINED_PRIMITIVES)
            .map_err(|_| RetainedPaintError::Capacity)?;
        Ok(Self {
            mode,
            scratch_pixels,
            scratch_animation_regions,
            primitives,
            layers,
            current_sample_rect: None,
            current_first: None,
            current_last: None,
            current_operations: 0,
            error: None,
            invalidated: Arc::new(AtomicBool::new(false)),
            coordinate_origin,
            current_support: None,
            current_writes,
        })
    }

    #[track_caller]
    pub(super) fn poison(&mut self) {
        trace_layer_mismatch("poison", || {
            format!(
                "origin={:?} caller={}",
                self.coordinate_origin,
                std::panic::Location::caller()
            )
        });
        self.error
            .get_or_insert(RetainedPaintError::TraversalMismatch);
        self.invalidated.store(true, Ordering::Relaxed);
    }

    #[track_caller]
    pub(super) fn mark_invalid(&self) {
        trace_layer_mismatch("mark_invalid", || {
            format!(
                "origin={:?} caller={}",
                self.coordinate_origin,
                std::panic::Location::caller()
            )
        });
        self.invalidated.store(true, Ordering::Relaxed);
    }

    pub(super) fn invalidation_guard(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.invalidated)
    }

    fn expected_start(
        &mut self,
        token: RetainedPrimitiveToken,
        write_rect: PixelRect,
    ) -> Option<bool> {
        let coordinate_origin = self.coordinate_origin;
        match &mut self.mode {
            RecorderMode::Plan | RecorderMode::FlatCapture => None,
            RecorderMode::Capture { expected, cursor } => {
                let planned = expected.get(*cursor).copied();
                *cursor = cursor.saturating_add(1);
                match planned {
                    Some(planned) if planned.token == token && planned.write_rect == write_rect => {
                        Some(planned.starts_layer)
                    }
                    _ => {
                        trace_layer_mismatch("primitive", || {
                            format!(
                                "cursor={} planned={planned:?} actual_token={token:?} actual_rect={write_rect:?} origin={:?}",
                                (*cursor).saturating_sub(1),
                                coordinate_origin
                            )
                        });
                        self.poison();
                        None
                    }
                }
            }
        }
    }

    fn begin(
        &mut self,
        kind: RetainedPrimitiveKind,
        rect: Rect,
    ) -> Result<(RetainedPrimitiveToken, PixelRect, PixelRect, bool), RetainedPaintError> {
        if self.primitives.len() >= MAX_RETAINED_PRIMITIVES {
            return Err(RetainedPaintError::Capacity);
        }
        let ordinal =
            u32::try_from(self.primitives.len()).map_err(|_| RetainedPaintError::Capacity)?;
        let token = RetainedPrimitiveToken { ordinal, kind };
        let local_write = PixelRect::write_bounds(rect)?;
        let write_rect = local_write.translated(self.coordinate_origin)?;
        let sample_rect = write_rect.sampling_support()?;
        let overlap = self.current_support.is_some_and(|current| {
            current.intersects(write_rect)
                && self
                    .current_writes
                    .iter()
                    .any(|previous| previous.intersects(write_rect))
        });
        let starts_layer = match self.expected_start(token, write_rect) {
            Some(expected) => {
                if expected != overlap {
                    return Err(RetainedPaintError::TraversalMismatch);
                }
                expected
            }
            None if self.error.is_some() => false,
            None => overlap,
        };
        self.primitives.push(PlannedPrimitive {
            token,
            starts_layer,
            write_rect,
        });
        Ok((token, sample_rect, write_rect, starts_layer))
    }

    fn extend_group(
        &mut self,
        token: RetainedPrimitiveToken,
        sample_rect: PixelRect,
        write_rect: PixelRect,
    ) {
        self.current_sample_rect = Some(
            self.current_sample_rect
                .map_or(sample_rect, |old| old.union(sample_rect)),
        );
        self.current_support = Some(
            self.current_support
                .map_or(write_rect, |old| old.union(write_rect)),
        );
        self.current_writes.push(write_rect);
        self.current_first.get_or_insert(token);
        self.current_last = Some(token);
        self.current_operations = self.current_operations.saturating_add(1);
    }

    fn clear_scratch(&mut self, width: u32, bounds: PixelRect) {
        if bounds.empty() {
            self.scratch_animation_regions.clear();
            return;
        }
        let stride = width as usize * 4;
        let left = bounds.left as usize * 4;
        let right = bounds.right as usize * 4;
        for y in bounds.top as usize..bounds.bottom as usize {
            self.scratch_pixels[y * stride + left..y * stride + right].fill(0);
        }
        self.scratch_animation_regions.clear();
    }

    fn seal(&mut self, width: u32, height: u32, capture: bool) -> Result<(), RetainedPaintError> {
        let (Some(sample_rect), Some(first), Some(last)) = (
            self.current_sample_rect.take(),
            self.current_first.take(),
            self.current_last.take(),
        ) else {
            return Ok(());
        };
        self.current_support = None;
        self.current_writes.clear();
        let inverse_origin = (
            self.coordinate_origin
                .0
                .checked_neg()
                .ok_or(RetainedPaintError::TraversalMismatch)?,
            self.coordinate_origin
                .1
                .checked_neg()
                .ok_or(RetainedPaintError::TraversalMismatch)?,
        );
        let local_sample = sample_rect.translated(inverse_origin)?;
        let crop = local_sample.clamp(width, height);
        let animation_count = self.scratch_animation_regions.len();
        let image = if capture && !crop.empty() {
            let mut scratch = Canvas {
                width,
                height,
                pixels: std::mem::take(&mut self.scratch_pixels),
                animation_regions: std::mem::take(&mut self.scratch_animation_regions),
                ink_probe: None,
                retained_layers: None,
                retained_operation_guard: None,
            };
            let cropped = copy_retained_region(&scratch, crop)?;
            if cropped.animation_regions.len() != animation_count {
                return Err(RetainedPaintError::TraversalMismatch);
            }
            self.scratch_pixels = std::mem::take(&mut scratch.pixels);
            self.scratch_animation_regions = std::mem::take(&mut scratch.animation_regions);
            Some(RetainedSparseImage {
                origin: (crop.left, crop.top),
                image: cropped.into_image(),
            })
        } else {
            None
        };
        self.layers.push(RetainedDocumentLayer {
            identity: RetainedLayerIdentity {
                first,
                last,
                operation_count: self.current_operations,
            },
            sample_rect,
            animation_count,
            image,
        });
        self.clear_scratch(width, crop);
        self.current_operations = 0;
        Ok(())
    }

    pub(super) fn finish(
        mut self,
        width: u32,
        height: u32,
    ) -> Result<(Vec<PlannedPrimitive>, Vec<RetainedDocumentLayer>), RetainedPaintError> {
        if let Some(error) = self.error.take() {
            trace_layer_mismatch("recorder_error", || {
                format!("error={error:?} origin={:?}", self.coordinate_origin)
            });
            return Err(error);
        }
        let capture = !matches!(self.mode, RecorderMode::Plan);
        self.seal(width, height, capture)?;
        if self.scratch_pixels.iter().any(|byte| *byte != 0)
            || !self.scratch_animation_regions.is_empty()
        {
            trace_layer_mismatch("scratch_not_empty", || {
                format!(
                    "pixels={} animations={} origin={:?}",
                    self.scratch_pixels
                        .iter()
                        .filter(|byte| **byte != 0)
                        .count(),
                    self.scratch_animation_regions.len(),
                    self.coordinate_origin
                )
            });
            return Err(RetainedPaintError::TraversalMismatch);
        }
        if let RecorderMode::Capture { expected, cursor } = &self.mode
            && *cursor != expected.len()
        {
            trace_layer_mismatch("primitive_count", || {
                format!(
                    "actual={cursor} expected={} origin={:?}",
                    expected.len(),
                    self.coordinate_origin
                )
            });
            return Err(RetainedPaintError::TraversalMismatch);
        }
        if let Some(error) = self.error {
            trace_layer_mismatch("recorder_error", || {
                format!("error={error:?} origin={:?}", self.coordinate_origin)
            });
            return Err(error);
        }
        if self.invalidated.load(Ordering::Relaxed) {
            trace_layer_mismatch("invalidated", || {
                format!("origin={:?}", self.coordinate_origin)
            });
            return Err(RetainedPaintError::TraversalMismatch);
        }
        if matches!(self.mode, RecorderMode::FlatCapture) {
            return Err(RetainedPaintError::TraversalMismatch);
        }
        Ok((self.primitives, self.layers))
    }

    fn finish_flat(
        mut self,
    ) -> Result<(Vec<u8>, Vec<super::animation::AnimationRegion>), RetainedPaintError> {
        if let Some(error) = self.error.take() {
            trace_layer_mismatch("recorder_error", || {
                format!("error={error:?} origin={:?}", self.coordinate_origin)
            });
            return Err(error);
        }
        if !matches!(self.mode, RecorderMode::FlatCapture) || !self.layers.is_empty() {
            trace_layer_mismatch("flat_state", || {
                format!(
                    "layers={} origin={:?}",
                    self.layers.len(),
                    self.coordinate_origin
                )
            });
            return Err(RetainedPaintError::TraversalMismatch);
        }
        if self.invalidated.load(Ordering::Relaxed) {
            trace_layer_mismatch("invalidated", || {
                format!("origin={:?}", self.coordinate_origin)
            });
            return Err(RetainedPaintError::TraversalMismatch);
        }
        let mut animation_regions = Vec::new();
        animation_regions
            .try_reserve_exact(self.scratch_animation_regions.len())
            .map_err(|_| RetainedPaintError::Capacity)?;
        animation_regions.extend(self.scratch_animation_regions);
        Ok((self.scratch_pixels, animation_regions))
    }
}

impl Canvas {
    pub(super) fn install_layer_planner(
        &mut self,
        coordinate_origin: (i32, i32),
    ) -> Result<Arc<AtomicBool>, RetainedPaintError> {
        if self.retained_layers.is_some() {
            return Err(RetainedPaintError::TraversalMismatch);
        }
        let recorder = CanvasLayerRecorder::planning(self.width, self.height, coordinate_origin)?;
        let guard = recorder.invalidation_guard();
        self.retained_layers = Some(Box::new(recorder));
        Ok(guard)
    }

    pub(super) fn install_layer_capture(
        &mut self,
        coordinate_origin: (i32, i32),
        expected: Arc<[PlannedPrimitive]>,
    ) -> Result<Arc<AtomicBool>, RetainedPaintError> {
        if self.retained_layers.is_some() {
            return Err(RetainedPaintError::TraversalMismatch);
        }
        let recorder =
            CanvasLayerRecorder::capturing(self.width, self.height, coordinate_origin, expected)?;
        let guard = recorder.invalidation_guard();
        self.retained_layers = Some(Box::new(recorder));
        Ok(guard)
    }

    pub(super) fn install_flat_layer_capture(
        &mut self,
        coordinate_origin: (i32, i32),
    ) -> Result<Arc<AtomicBool>, RetainedPaintError> {
        if self.retained_layers.is_some() {
            return Err(RetainedPaintError::TraversalMismatch);
        }
        let recorder =
            CanvasLayerRecorder::capturing_flat(self.width, self.height, coordinate_origin)?;
        let guard = recorder.invalidation_guard();
        self.retained_layers = Some(Box::new(recorder));
        Ok(guard)
    }

    #[track_caller]
    pub(super) fn poison_retained_layers(&mut self) {
        if let Some(recorder) = &mut self.retained_layers {
            recorder.poison();
        }
    }

    #[track_caller]
    pub(super) fn invalidate_retained_layers(&self) {
        if let Some(recorder) = &self.retained_layers {
            recorder.mark_invalid();
        }
        if let Some(guard) = &self.retained_operation_guard {
            guard.store(true, Ordering::Relaxed);
        }
    }

    pub(super) fn permit_retained_animation_region(&self, current_len: usize) -> bool {
        if let Some(recorder) = &self.retained_layers {
            recorder.mark_invalid();
            return false;
        }
        let Some(guard) = &self.retained_operation_guard else {
            return true;
        };
        if current_len < MAX_RETAINED_ANIMATION_REGIONS {
            true
        } else {
            guard.store(true, Ordering::Relaxed);
            false
        }
    }

    pub(super) fn with_retained_operation(
        &mut self,
        kind: RetainedPrimitiveKind,
        dirty: Rect,
        paint: impl FnOnce(&mut Canvas),
    ) -> bool {
        let Some(mut recorder) = self.retained_layers.take() else {
            return false;
        };
        let started = recorder.begin(kind, dirty);
        let Ok((token, bounds, write_rect, starts_layer)) = started else {
            recorder.error = started.err();
            self.retained_layers = Some(recorder);
            return true;
        };
        if starts_layer && matches!(recorder.mode, RecorderMode::FlatCapture) {
            recorder.error = Some(RetainedPaintError::TraversalMismatch);
            self.retained_layers = Some(recorder);
            return true;
        }
        let capture = !matches!(recorder.mode, RecorderMode::Plan);
        if starts_layer {
            if let Err(error) = recorder.seal(self.width, self.height, capture) {
                recorder.error = Some(error);
                self.retained_layers = Some(recorder);
                return true;
            }
        }
        if self.retained_operation_guard.is_some() {
            recorder.poison();
            self.retained_layers = Some(recorder);
            return true;
        }
        self.retained_operation_guard = Some(recorder.invalidation_guard());
        std::mem::swap(&mut self.pixels, &mut recorder.scratch_pixels);
        std::mem::swap(
            &mut self.animation_regions,
            &mut recorder.scratch_animation_regions,
        );
        paint(self);
        self.retained_operation_guard = None;
        std::mem::swap(&mut self.pixels, &mut recorder.scratch_pixels);
        std::mem::swap(
            &mut self.animation_regions,
            &mut recorder.scratch_animation_regions,
        );
        recorder.extend_group(token, bounds, write_rect);
        self.retained_layers = Some(recorder);
        true
    }

    pub(super) fn take_retained_layers(
        &mut self,
    ) -> Result<Option<(Vec<PlannedPrimitive>, Vec<RetainedDocumentLayer>)>, RetainedPaintError>
    {
        self.take_retained_layers_with_base(false)
    }

    pub(super) fn take_flat_layer_capture(&mut self) -> Result<bool, RetainedPaintError> {
        let Some(recorder) = self.retained_layers.take() else {
            return Ok(false);
        };
        if self.pixels.iter().any(|byte| *byte != 0) || !self.animation_regions.is_empty() {
            return Err(RetainedPaintError::TraversalMismatch);
        }
        let (pixels, animation_regions) = recorder.finish_flat()?;
        self.pixels = pixels;
        self.animation_regions = animation_regions;
        Ok(true)
    }

    /// Finishes planning while transferring a single unsealed group's pixels
    /// back to the public canvas. Multiple groups still require capture mode.
    pub(super) fn take_retained_layers_for_plan(
        &mut self,
    ) -> Result<Option<(Vec<PlannedPrimitive>, Vec<RetainedDocumentLayer>)>, RetainedPaintError>
    {
        self.take_retained_layers_with_options(false, true)
    }

    pub(super) fn take_retained_layers_with_base(
        &mut self,
        preserve_base: bool,
    ) -> Result<Option<(Vec<PlannedPrimitive>, Vec<RetainedDocumentLayer>)>, RetainedPaintError>
    {
        self.take_retained_layers_with_options(preserve_base, false)
    }

    fn take_retained_layers_with_options(
        &mut self,
        preserve_base: bool,
        reuse_flat_planning_pixels: bool,
    ) -> Result<Option<(Vec<PlannedPrimitive>, Vec<RetainedDocumentLayer>)>, RetainedPaintError>
    {
        let Some(mut recorder) = self.retained_layers.take() else {
            return Ok(None);
        };
        if (!preserve_base && self.pixels.iter().any(|byte| *byte != 0))
            || !self.animation_regions.is_empty()
        {
            trace_layer_mismatch("public_canvas", || {
                format!(
                    "preserve_base={preserve_base} pixels={} animations={} size={}x{}",
                    self.pixels.iter().filter(|byte| **byte != 0).count(),
                    self.animation_regions.len(),
                    self.width,
                    self.height
                )
            });
            return Err(RetainedPaintError::TraversalMismatch);
        }
        if reuse_flat_planning_pixels
            && matches!(recorder.mode, RecorderMode::Plan)
            && recorder.layers.is_empty()
        {
            // No earlier group has sealed, so scratch holds the entire run.
            // Give the recorder the already-empty public surface before finish
            // seals metadata and checks guards. Only playback metadata is copied;
            // its peak is covered by recorder_metadata_peak_bytes.
            let mut animations = Vec::new();
            animations
                .try_reserve_exact(recorder.scratch_animation_regions.len())
                .map_err(|_| RetainedPaintError::Capacity)?;
            animations.extend_from_slice(&recorder.scratch_animation_regions);
            std::mem::swap(&mut self.pixels, &mut recorder.scratch_pixels);
            self.animation_regions = animations;
        }
        recorder.finish(self.width, self.height).map(Some)
    }
}
