//! Bounded, paint-order-preserving extraction of viewport-fixed raster runs.
//!
//! This first-stage collector owns only pixels and numeric metadata. It does not
//! publish overscan coverage or connect the retained result to the GUI.

use super::retained_layers::{
    PixelRect, PlannedLayer, RetainedDocumentLayer, RetainedLayerIdentity, RetainedLayerPlan,
    RetainedRunLayerPlan, RetainedSparseImage, layer_plan_metadata_max_bytes,
    recorder_metadata_peak_bytes,
};
use super::*;

mod diagnostics;
mod filter_source;
pub(crate) use diagnostics::{CrossSpaceClipDiagnostic, diagnose_cross_space_clip};
use filter_source::{FilterRecipe, FilterSourceLayer, FilterSourcePlan};

const MAX_RETAINED_RUNS: usize = 64;
const MAX_RETAINED_BYTES: usize = 64 * 1024 * 1024;
const MAX_VIEWPORT_FIXED_ROOTS: usize = (MAX_RETAINED_RUNS - 1) / 2;
const MAX_RETAINED_TILES: usize = 256;
const MAX_RETAINED_LAYER_SAMPLES: usize = 4_096;
const MAX_RETAINED_LAYER_COMPOSITE_TAPS: usize = 67_108_864;
const MAX_VERTICAL_OVERSCAN_SAMPLES: usize = 7;
const MAX_EXACT_F32_INTEGER: i64 = 1 << 24;

fn trace_layer_mismatch(stage: &str, detail: impl FnOnce() -> String) {
    if std::env::var_os("OMOIKANE_TRACE_RETAINED_LAYERS").is_some() {
        eprintln!(
            "OMOIKANE_RETAINED_LAYER_MISMATCH stage={stage} {}",
            detail()
        );
    }
}

/// Coordinate space of one extracted full-viewport raster run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RetainedPaintSpace {
    Document,
    ViewportFixed,
}

/// Stable per-capture traversal identity for a paint-order run. Fixed ordinals
/// come from logical preorder; document ordinals count painter flushes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RetainedPaintRunIdentity {
    Document(usize),
    ViewportFixed(usize),
    FilterSource(usize),
}

/// One owned full-viewport raster in original paint order.
pub(crate) struct RetainedPaintRun {
    pub(crate) space: RetainedPaintSpace,
    pub(crate) identity: RetainedPaintRunIdentity,
    pub(crate) image: Image,
    document_layers: Vec<RetainedDocumentLayer>,
    filter_source: Option<FilterSourceLayer>,
}

/// Owned result of a successful retained-paint extraction.
pub(crate) struct RetainedPaintFrame {
    width: u32,
    height: u32,
    runs: Vec<RetainedPaintRun>,
}

/// One full-viewport tile positioned on the integer document raster.
pub(crate) struct RetainedOverscanTile {
    origin: (i32, i32),
    image: Image,
}

impl RetainedOverscanTile {
    pub(crate) fn origin(&self) -> (i32, i32) {
        self.origin
    }

    pub(crate) fn image(&self) -> &Image {
        &self.image
    }
}

/// One paint-order run containing document tiles or one viewport-fixed tile.
pub(crate) struct RetainedOverscanRun {
    space: RetainedPaintSpace,
    identity: RetainedPaintRunIdentity,
    tiles: Vec<RetainedOverscanTile>,
    sample_coverages: Vec<OverscanRasterRect>,
    document_layers: Vec<RetainedOverscanLayer>,
    filter_source: Option<FilterSourceLayer>,
}

struct RetainedOverscanLayer {
    identity: RetainedLayerIdentity,
    sample_rect: PixelRect,
    samples: Vec<RetainedLayerSample>,
}

struct RetainedLayerSample {
    coverage: OverscanRasterRect,
    tile: Option<RetainedOverscanTile>,
}

impl RetainedOverscanRun {
    pub(crate) fn space(&self) -> RetainedPaintSpace {
        self.space
    }

    pub(crate) fn identity(&self) -> RetainedPaintRunIdentity {
        self.identity
    }

    pub(crate) fn tiles(&self) -> &[RetainedOverscanTile] {
        &self.tiles
    }

    #[cfg(test)]
    pub(crate) fn filter_source_sample_count(&self) -> Option<usize> {
        self.filter_source
            .as_ref()
            .map(FilterSourceLayer::sample_count)
    }
}

/// Owned vertical document coverage with viewport-fixed paint-order runs.
pub(crate) struct RetainedOverscanFrame {
    width: u32,
    height: u32,
    raster_scale: f64,
    document_x: i32,
    first_y: i32,
    last_y: i32,
    current_y: i32,
    runs: Vec<RetainedOverscanRun>,
    empty_filter_sources: Vec<FilterRecipe>,
}

impl RetainedOverscanFrame {
    pub(crate) fn coverage(&self) -> ((i32, i32), (i32, i32)) {
        (
            (self.document_x, self.first_y),
            (self.document_x, self.last_y),
        )
    }

    pub(crate) fn current_scroll(&self) -> (i32, i32) {
        (self.document_x, self.current_y)
    }

    pub(crate) fn logical_coverage(&self) -> ((f64, f64), (f64, f64)) {
        (
            (f64::from(self.document_x), f64::from(self.first_y)),
            (f64::from(self.document_x), f64::from(self.last_y)),
        )
    }

    pub(crate) fn validate_compositor(&self, raster_scale: f64) -> Result<(), RetainedPaintError> {
        if !raster_scale.is_finite() || raster_scale <= 0.0 || self.raster_scale != raster_scale {
            return Err(RetainedPaintError::InvalidOverscanRasterScale);
        }
        if self.width == 0
            || self.height == 0
            || self.first_y > self.last_y
            || self.current_y < self.first_y
            || self.current_y > self.last_y
        {
            return Err(RetainedPaintError::InvalidOverscanFrame);
        }
        let required = required_overscan_raster_rect(
            self.width,
            self.height,
            self.logical_coverage(),
            raster_scale,
        )
        .ok_or(RetainedPaintError::InvalidOverscanFrame)?;
        let mut has_document_run = false;
        for run in &self.runs {
            if let Some(source) = &run.filter_source {
                source.validate(required)?;
                continue;
            }
            validate_overscan_tiles(run)?;
            if run.space == RetainedPaintSpace::Document {
                has_document_run = true;
                if !overscan_rects_cover(&run.sample_coverages, required)
                    || run.document_layers.iter().any(|layer| {
                        let coverage = layer
                            .samples
                            .iter()
                            .map(|sample| sample.coverage)
                            .collect::<Vec<_>>();
                        !overscan_rects_cover(&coverage, required)
                    })
                {
                    return Err(RetainedPaintError::IncompleteOverscanCoverage);
                }
            }
        }
        if !has_document_run {
            return Err(RetainedPaintError::InvalidOverscanFrame);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn empty_filter_source_count(&self) -> usize {
        self.empty_filter_sources.len()
    }

    #[cfg(test)]
    pub(crate) fn filter_source_storage_bytes(&self) -> usize {
        self.runs
            .iter()
            .filter_map(|run| run.filter_source.as_ref())
            .map(FilterSourceLayer::bytes)
            .sum()
    }

    pub(crate) fn runs(&self) -> &[RetainedOverscanRun] {
        &self.runs
    }

    #[cfg(test)]
    pub(crate) fn from_test_parts(
        width: u32,
        height: u32,
        raster_scale: f64,
        coverage: ((i32, i32), (i32, i32)),
        current_scroll: (i32, i32),
        runs: Vec<(RetainedPaintSpace, Vec<((i32, i32), Image)>)>,
    ) -> Self {
        let runs = runs
            .into_iter()
            .enumerate()
            .map(|(index, (space, tiles))| RetainedOverscanRun {
                identity: match space {
                    RetainedPaintSpace::Document => RetainedPaintRunIdentity::Document(index),
                    RetainedPaintSpace::ViewportFixed => {
                        RetainedPaintRunIdentity::ViewportFixed(index)
                    }
                },
                space,
                filter_source: None,
                document_layers: Vec::new(),
                sample_coverages: tiles
                    .iter()
                    .map(|(origin, image)| OverscanRasterRect {
                        left: i64::from(origin.0),
                        top: i64::from(origin.1),
                        right: i64::from(origin.0) + i64::from(image.width()),
                        bottom: i64::from(origin.1) + i64::from(image.height()),
                    })
                    .collect(),
                tiles: tiles
                    .into_iter()
                    .map(|(origin, image)| RetainedOverscanTile { origin, image })
                    .collect(),
            })
            .collect();
        Self {
            width,
            height,
            raster_scale,
            document_x: coverage.0.0,
            first_y: coverage.0.1,
            last_y: coverage.1.1,
            current_y: current_scroll.1,
            runs,
            empty_filter_sources: Vec::new(),
        }
    }

    /// Re-composes an integer scroll inside retained vertical coverage.
    pub(crate) fn compose_integer(&self, scroll: (i32, i32)) -> Result<Canvas, RetainedPaintError> {
        if scroll.0 != self.document_x || scroll.1 < self.first_y || scroll.1 > self.last_y {
            return Err(RetainedPaintError::OutsideOverscanCoverage);
        }
        let mut canvas = Canvas::new(self.width, self.height);
        for run in &self.runs {
            if let Some(source) = &run.filter_source {
                source.compose(&mut canvas, (scroll.0 as f64, scroll.1 as f64))?;
                continue;
            }
            for tile in &run.tiles {
                let destination = match run.space {
                    RetainedPaintSpace::Document => {
                        (tile.origin.0 - scroll.0, tile.origin.1 - scroll.1)
                    }
                    RetainedPaintSpace::ViewportFixed => (0, 0),
                };
                canvas.draw_image(&tile.image, destination.0 as f32, destination.1 as f32);
            }
            for layer in &run.document_layers {
                for sample in &layer.samples {
                    let Some(tile) = &sample.tile else {
                        continue;
                    };
                    let destination = match run.space {
                        RetainedPaintSpace::Document => {
                            (tile.origin.0 - scroll.0, tile.origin.1 - scroll.1)
                        }
                        RetainedPaintSpace::ViewportFixed => tile.origin,
                    };
                    let coverage = translated_layer_coverage(sample.coverage, run.space, scroll);
                    canvas.draw_image_scaled_clipped(
                        &tile.image,
                        Rect {
                            x: destination.0 as f32,
                            y: destination.1 as f32,
                            width: tile.image.width() as f32,
                            height: tile.image.height() as f32,
                        },
                        Some(coverage),
                    );
                }
            }
        }
        Ok(canvas)
    }

    /// Re-composes a logical CSS scroll from owned raster tiles. Document runs
    /// are bilinearly sampled as one union; viewport-fixed runs remain unmoved.
    pub(crate) fn compose_logical(
        &self,
        scroll: (f64, f64),
        raster_scale: f64,
    ) -> Result<Canvas, RetainedPaintError> {
        if !scroll.0.is_finite() || !scroll.1.is_finite() {
            return Err(RetainedPaintError::InvalidOverscanScroll);
        }
        if self.raster_scale != raster_scale || !raster_scale.is_finite() || raster_scale <= 0.0 {
            return Err(RetainedPaintError::InvalidOverscanRasterScale);
        }
        let coverage = self.logical_coverage();
        if scroll.0 < coverage.0.0
            || scroll.0 > coverage.1.0
            || scroll.1 < coverage.0.1
            || scroll.1 > coverage.1.1
        {
            return Err(RetainedPaintError::OutsideOverscanCoverage);
        }
        let translation = OverscanSamplingTranslation::new(scroll, raster_scale)?;
        required_overscan_raster_rect(self.width, self.height, (scroll, scroll), raster_scale)
            .ok_or(RetainedPaintError::InvalidOverscanScroll)?;

        let mut canvas = Canvas::new(self.width, self.height);
        for run in &self.runs {
            if let Some(source) = &run.filter_source {
                source.compose(&mut canvas, scroll)?;
                continue;
            }
            match run.space {
                RetainedPaintSpace::ViewportFixed => {
                    for tile in &run.tiles {
                        canvas.draw_image(&tile.image, 0.0, 0.0);
                    }
                    for layer in &run.document_layers {
                        for sample in &layer.samples {
                            let Some(tile) = &sample.tile else {
                                continue;
                            };
                            canvas.draw_image_scaled_clipped(
                                &tile.image,
                                Rect {
                                    x: tile.origin.0 as f32,
                                    y: tile.origin.1 as f32,
                                    width: tile.image.width() as f32,
                                    height: tile.image.height() as f32,
                                },
                                Some(translated_layer_coverage(
                                    sample.coverage,
                                    run.space,
                                    (0, 0),
                                )),
                            );
                        }
                    }
                }
                RetainedPaintSpace::Document if translation.is_integer() => {
                    for tile in &run.tiles {
                        let x = i64::from(tile.origin.0) - translation.base_x;
                        let y = i64::from(tile.origin.1) - translation.base_y;
                        canvas.draw_image(&tile.image, x as f32, y as f32);
                    }
                    for layer in &run.document_layers {
                        for sample in &layer.samples {
                            let Some(tile) = &sample.tile else {
                                continue;
                            };
                            let x = i64::from(tile.origin.0) - translation.base_x;
                            let y = i64::from(tile.origin.1) - translation.base_y;
                            canvas.draw_image_scaled_clipped(
                                &tile.image,
                                Rect {
                                    x: x as f32,
                                    y: y as f32,
                                    width: tile.image.width() as f32,
                                    height: tile.image.height() as f32,
                                },
                                Some(translated_layer_coverage_f64(
                                    sample.coverage,
                                    translation.base_x,
                                    translation.base_y,
                                )),
                            );
                        }
                    }
                }
                RetainedPaintSpace::Document => {
                    match run.document_layers.as_slice() {
                        [] => composite_fractional_overscan_run(&mut canvas, run, translation),
                        [layer] if run.tiles.is_empty() => {
                            composite_fractional_overscan_layer(&mut canvas, layer, translation)
                        }
                        _ => composite_fractional_layered_run(&mut canvas, run, translation),
                    }
                    record_fractional_overscan_animation(&mut canvas, run, scroll, raster_scale);
                    for layer in &run.document_layers {
                        record_fractional_overscan_layer_animation(
                            &mut canvas,
                            layer,
                            scroll,
                            raster_scale,
                        );
                    }
                }
            }
        }
        Ok(canvas)
    }
}

const MAX_EXACT_F64_INTEGER: f64 = 9_007_199_254_740_991.0;
const MAX_EXACT_F64_INTEGER_I64: i64 = 9_007_199_254_740_991;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OverscanRasterRect {
    left: i64,
    top: i64,
    right: i64,
    bottom: i64,
}

impl OverscanRasterRect {
    fn overlaps(self, other: Self) -> bool {
        self.left < other.right
            && self.right > other.left
            && self.top < other.bottom
            && self.bottom > other.top
    }
}

fn translated_layer_coverage(
    coverage: OverscanRasterRect,
    space: RetainedPaintSpace,
    scroll: (i32, i32),
) -> Rect {
    let (x, y) = match space {
        RetainedPaintSpace::Document => (scroll.0, scroll.1),
        RetainedPaintSpace::ViewportFixed => (0, 0),
    };
    translated_layer_coverage_f64(coverage, i64::from(x), i64::from(y))
}

fn translated_layer_coverage_f64(
    coverage: OverscanRasterRect,
    scroll_x: i64,
    scroll_y: i64,
) -> Rect {
    Rect {
        x: (coverage.left - scroll_x) as f32,
        y: (coverage.top - scroll_y) as f32,
        width: (coverage.right - coverage.left) as f32,
        height: (coverage.bottom - coverage.top) as f32,
    }
}

#[derive(Clone, Copy)]
struct OverscanSamplingTranslation {
    base_x: i64,
    base_y: i64,
    fraction_x: f64,
    fraction_y: f64,
}

impl OverscanSamplingTranslation {
    fn new(scroll: (f64, f64), raster_scale: f64) -> Result<Self, RetainedPaintError> {
        let x = scroll.0 * raster_scale;
        let y = scroll.1 * raster_scale;
        let base_x = checked_floor_i64(x).ok_or(RetainedPaintError::InvalidOverscanScroll)?;
        let base_y = checked_floor_i64(y).ok_or(RetainedPaintError::InvalidOverscanScroll)?;
        Ok(Self {
            base_x,
            base_y,
            fraction_x: x - base_x as f64,
            fraction_y: y - base_y as f64,
        })
    }

    fn is_integer(self) -> bool {
        self.fraction_x == 0.0 && self.fraction_y == 0.0
    }
}

fn checked_floor_i64(value: f64) -> Option<i64> {
    let value = value.floor();
    (value.is_finite() && (-MAX_EXACT_F64_INTEGER..=MAX_EXACT_F64_INTEGER).contains(&value))
        .then(|| value as i64)
}

fn checked_ceil_i64(value: f64) -> Option<i64> {
    let value = value.ceil();
    (value.is_finite() && (-MAX_EXACT_F64_INTEGER..=MAX_EXACT_F64_INTEGER).contains(&value))
        .then(|| value as i64)
}

fn checked_filter_edge(offset: i64, extent: u32) -> Option<i64> {
    let edge = offset.checked_add(i64::from(extent.checked_sub(1)?))?;
    (-MAX_EXACT_F64_INTEGER_I64..=MAX_EXACT_F64_INTEGER_I64)
        .contains(&edge)
        .then_some(edge)
}

fn required_overscan_raster_rect(
    width: u32,
    height: u32,
    coverage: ((f64, f64), (f64, f64)),
    raster_scale: f64,
) -> Option<OverscanRasterRect> {
    let left = checked_floor_i64(coverage.0.0 * raster_scale)?;
    let top = checked_floor_i64(coverage.0.1 * raster_scale)?;
    let inclusive_right =
        checked_filter_edge(checked_ceil_i64(coverage.1.0 * raster_scale)?, width)?;
    let inclusive_bottom =
        checked_filter_edge(checked_ceil_i64(coverage.1.1 * raster_scale)?, height)?;
    Some(OverscanRasterRect {
        left,
        top,
        right: inclusive_right.checked_add(1)?,
        bottom: inclusive_bottom.checked_add(1)?,
    })
}

fn overscan_tile_rect(tile: &RetainedOverscanTile) -> OverscanRasterRect {
    OverscanRasterRect {
        left: i64::from(tile.origin.0),
        top: i64::from(tile.origin.1),
        right: i64::from(tile.origin.0) + i64::from(tile.image.width()),
        bottom: i64::from(tile.origin.1) + i64::from(tile.image.height()),
    }
}

fn validate_overscan_tiles(run: &RetainedOverscanRun) -> Result<(), RetainedPaintError> {
    for (index, coverage) in run.sample_coverages.iter().enumerate() {
        if coverage.left >= coverage.right
            || coverage.top >= coverage.bottom
            || run.sample_coverages[..index]
                .iter()
                .any(|previous| previous.overlaps(*coverage))
        {
            return Err(RetainedPaintError::OverlappingOverscanTiles);
        }
    }
    for (index, tile) in run.tiles.iter().enumerate() {
        if tile.image.width() == 0 || tile.image.height() == 0 {
            return Err(RetainedPaintError::InvalidOverscanFrame);
        }
        if run.tiles[..index]
            .iter()
            .any(|previous| overscan_tile_rect(previous).overlaps(overscan_tile_rect(tile)))
        {
            return Err(RetainedPaintError::OverlappingOverscanTiles);
        }
        if !run.sample_coverages.iter().any(|coverage| {
            let tile = overscan_tile_rect(tile);
            coverage.left <= tile.left
                && coverage.top <= tile.top
                && coverage.right >= tile.right
                && coverage.bottom >= tile.bottom
        }) {
            return Err(RetainedPaintError::InvalidOverscanFrame);
        }
    }
    for layer in &run.document_layers {
        if layer.samples.len() != run.sample_coverages.len()
            || layer
                .samples
                .iter()
                .zip(&run.sample_coverages)
                .any(|(sample, coverage)| sample.coverage != *coverage)
        {
            return Err(RetainedPaintError::InvalidOverscanFrame);
        }
    }
    Ok(())
}

fn overscan_rects_cover(rects: &[OverscanRasterRect], required: OverscanRasterRect) -> bool {
    let mut ys = vec![required.top, required.bottom];
    for rect in rects {
        ys.push(rect.top.clamp(required.top, required.bottom));
        ys.push(rect.bottom.clamp(required.top, required.bottom));
    }
    ys.sort_unstable();
    ys.dedup();
    for slab in ys.windows(2) {
        let mut intervals = rects
            .iter()
            .filter_map(|rect| {
                (rect.top <= slab[0] && rect.bottom >= slab[1]).then_some((rect.left, rect.right))
            })
            .collect::<Vec<_>>();
        intervals.sort_unstable();
        let mut covered = required.left;
        for (left, right) in intervals {
            if left > covered {
                break;
            }
            covered = covered.max(right);
            if covered >= required.right {
                break;
            }
        }
        if covered < required.right {
            return false;
        }
    }
    true
}

fn overscan_run_pixel(run: &RetainedOverscanRun, x: i64, y: i64) -> [u8; 4] {
    for tile in &run.tiles {
        let rect = overscan_tile_rect(tile);
        if x < rect.left || x >= rect.right || y < rect.top || y >= rect.bottom {
            continue;
        }
        let local_x = (x - rect.left) as usize;
        let local_y = (y - rect.top) as usize;
        let index = (local_y * tile.image.width() as usize + local_x) * 4;
        let mut pixel = [0; 4];
        pixel.copy_from_slice(&tile.image.pixels()[index..index + 4]);
        return pixel;
    }
    [0; 4]
}

fn overscan_layer_pixel(layer: &RetainedOverscanLayer, x: i64, y: i64) -> [u8; 4] {
    if x < i64::from(layer.sample_rect.left)
        || x >= i64::from(layer.sample_rect.right)
        || y < i64::from(layer.sample_rect.top)
        || y >= i64::from(layer.sample_rect.bottom)
    {
        return [0; 4];
    }
    for sample in &layer.samples {
        if x < sample.coverage.left
            || x >= sample.coverage.right
            || y < sample.coverage.top
            || y >= sample.coverage.bottom
        {
            continue;
        }
        let Some(tile) = &sample.tile else {
            return [0; 4];
        };
        let rect = overscan_tile_rect(tile);
        if x < rect.left || x >= rect.right || y < rect.top || y >= rect.bottom {
            return [0; 4];
        }
        let local_x = (x - rect.left) as usize;
        let local_y = (y - rect.top) as usize;
        let index = (local_y * tile.image.width() as usize + local_x) * 4;
        let mut pixel = [0; 4];
        pixel.copy_from_slice(&tile.image.pixels()[index..index + 4]);
        return pixel;
    }
    [0; 4]
}

fn sample_overscan_run(
    run: &RetainedOverscanRun,
    x0: i64,
    y0: i64,
    translation: OverscanSamplingTranslation,
) -> [u8; 4] {
    let x1 = if translation.fraction_x == 0.0 {
        x0
    } else {
        x0 + 1
    };
    let y1 = if translation.fraction_y == 0.0 {
        y0
    } else {
        y0 + 1
    };
    let samples = [
        overscan_run_pixel(run, x0, y0),
        overscan_run_pixel(run, x1, y0),
        overscan_run_pixel(run, x0, y1),
        overscan_run_pixel(run, x1, y1),
    ];
    interpolate_overscan_samples(samples, translation.fraction_x, translation.fraction_y)
}

fn fractional_sample_points(
    x0: i64,
    y0: i64,
    translation: OverscanSamplingTranslation,
) -> [(i64, i64); 4] {
    let x1 = if translation.fraction_x == 0.0 {
        x0
    } else {
        x0 + 1
    };
    let y1 = if translation.fraction_y == 0.0 {
        y0
    } else {
        y0 + 1
    };
    [(x0, y0), (x1, y0), (x0, y1), (x1, y1)]
}

fn sample_overscan_layer(
    layer: &RetainedOverscanLayer,
    x0: i64,
    y0: i64,
    translation: OverscanSamplingTranslation,
) -> [u8; 4] {
    let samples = fractional_sample_points(x0, y0, translation)
        .map(|(x, y)| overscan_layer_pixel(layer, x, y));
    interpolate_overscan_samples(samples, translation.fraction_x, translation.fraction_y)
}

fn interpolate_overscan_samples(
    samples: [[u8; 4]; 4],
    fraction_x: f64,
    fraction_y: f64,
) -> [u8; 4] {
    let weights = [
        (1.0 - fraction_x) * (1.0 - fraction_y),
        fraction_x * (1.0 - fraction_y),
        (1.0 - fraction_x) * fraction_y,
        fraction_x * fraction_y,
    ];
    let alpha = samples
        .iter()
        .zip(weights)
        .map(|(sample, weight)| f64::from(sample[3]) * weight)
        .sum::<f64>();
    if alpha == 0.0 {
        return [0; 4];
    }
    let mut result = [0; 4];
    for channel in 0..3 {
        let premultiplied = samples
            .iter()
            .zip(weights)
            .map(|(sample, weight)| f64::from(sample[channel]) * f64::from(sample[3]) * weight)
            .sum::<f64>();
        result[channel] = quantize_overscan_channel(premultiplied / alpha);
    }
    result[3] = quantize_overscan_channel(alpha);
    result
}

fn quantize_overscan_channel(value: f64) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

fn composite_fractional_overscan_run(
    canvas: &mut Canvas,
    run: &RetainedOverscanRun,
    translation: OverscanSamplingTranslation,
) {
    for y in 0..canvas.height() {
        let source_y = translation.base_y + i64::from(y);
        for x in 0..canvas.width() {
            let source_x = translation.base_x + i64::from(x);
            let pixel = sample_overscan_run(run, source_x, source_y, translation);
            canvas.blend_pixel(x, y, Color::rgba(pixel[0], pixel[1], pixel[2], pixel[3]));
        }
    }
}

fn composite_fractional_overscan_layer(
    canvas: &mut Canvas,
    layer: &RetainedOverscanLayer,
    translation: OverscanSamplingTranslation,
) {
    for y in 0..canvas.height() {
        let source_y = translation.base_y + i64::from(y);
        for x in 0..canvas.width() {
            let source_x = translation.base_x + i64::from(x);
            let pixel = sample_overscan_layer(layer, source_x, source_y, translation);
            canvas.blend_pixel(x, y, Color::rgba(pixel[0], pixel[1], pixel[2], pixel[3]));
        }
    }
}

fn composite_fractional_layered_run(
    canvas: &mut Canvas,
    run: &RetainedOverscanRun,
    translation: OverscanSamplingTranslation,
) {
    for y in 0..canvas.height() {
        let source_y = translation.base_y + i64::from(y);
        for x in 0..canvas.width() {
            let source_x = translation.base_x + i64::from(x);
            let backdrop = canvas.pixel(x, y).unwrap_or(Color::rgba(0, 0, 0, 0));
            let mut samples = [[backdrop.r, backdrop.g, backdrop.b, backdrop.a]; 4];
            let sample_points = fractional_sample_points(source_x, source_y, translation);
            for (sample, (tap_x, tap_y)) in samples.iter_mut().zip(sample_points) {
                let source = overscan_run_pixel(run, tap_x, tap_y);
                blend_pixel(
                    sample,
                    Color::rgba(source[0], source[1], source[2], source[3]),
                );
            }
            for layer in &run.document_layers {
                if sample_points.iter().all(|&(tap_x, tap_y)| {
                    tap_x < i64::from(layer.sample_rect.left)
                        || tap_x >= i64::from(layer.sample_rect.right)
                        || tap_y < i64::from(layer.sample_rect.top)
                        || tap_y >= i64::from(layer.sample_rect.bottom)
                }) {
                    continue;
                }
                for (sample, (tap_x, tap_y)) in samples.iter_mut().zip(sample_points) {
                    let source = overscan_layer_pixel(layer, tap_x, tap_y);
                    blend_pixel(
                        sample,
                        Color::rgba(source[0], source[1], source[2], source[3]),
                    );
                }
            }
            let pixel = interpolate_overscan_samples(
                samples,
                translation.fraction_x,
                translation.fraction_y,
            );
            canvas.set_pixel(x, y, Color::rgba(pixel[0], pixel[1], pixel[2], pixel[3]));
        }
    }
}

fn record_fractional_overscan_layer_animation(
    canvas: &mut Canvas,
    layer: &RetainedOverscanLayer,
    scroll: (f64, f64),
    raster_scale: f64,
) {
    let clip = Rect {
        x: 0.0,
        y: 0.0,
        width: canvas.width() as f32,
        height: canvas.height() as f32,
    };
    for sample in &layer.samples {
        let Some(tile) = &sample.tile else {
            continue;
        };
        let destination = Rect {
            x: (f64::from(tile.origin.0) - scroll.0 * raster_scale) as f32,
            y: (f64::from(tile.origin.1) - scroll.1 * raster_scale) as f32,
            width: tile.image.width() as f32,
            height: tile.image.height() as f32,
        };
        canvas.record_animation_regions_from_scaled_image(&tile.image, destination, clip);
    }
}

fn record_fractional_overscan_animation(
    canvas: &mut Canvas,
    run: &RetainedOverscanRun,
    scroll: (f64, f64),
    raster_scale: f64,
) {
    let clip = Rect {
        x: 0.0,
        y: 0.0,
        width: canvas.width() as f32,
        height: canvas.height() as f32,
    };
    for tile in &run.tiles {
        let destination = Rect {
            x: (f64::from(tile.origin.0) - scroll.0 * raster_scale) as f32,
            y: (f64::from(tile.origin.1) - scroll.1 * raster_scale) as f32,
            width: tile.image.width() as f32,
            height: tile.image.height() as f32,
        };
        canvas.record_animation_regions_from_scaled_image(&tile.image, destination, clip);
    }
}

#[derive(Clone, Debug)]
struct ResolvedVerticalOverscanPlan {
    document_x: i32,
    sample_y: Vec<i32>,
    current_index: usize,
    final_tiles: usize,
    empty_filter_sources: Vec<FilterRecipe>,
    scratch_bytes: usize,
    metadata_bytes: usize,
}

/// Integer, vertically contiguous capture request. Each sample is one viewport
/// height after the previous sample; `current_index` identifies fallback paint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct VerticalOverscanPlan {
    document_x: i32,
    first_y: i32,
    sample_count: u8,
    current_index: u8,
}

impl VerticalOverscanPlan {
    pub(crate) fn new(
        document_x: i32,
        first_y: i32,
        sample_count: u8,
        current_index: u8,
    ) -> Result<Self, RetainedPaintError> {
        if document_x < 0
            || first_y < 0
            || sample_count == 0
            || usize::from(sample_count) > MAX_VERTICAL_OVERSCAN_SAMPLES
            || current_index >= sample_count
        {
            return Err(RetainedPaintError::InvalidOverscanPlan);
        }
        Ok(Self {
            document_x,
            first_y,
            sample_count,
            current_index,
        })
    }

    fn resolve(
        self,
        viewport: Rect,
        dimensions: SurfaceDimensions,
        expected_runs: usize,
        sources: Option<&FilterSourcePlan>,
        layers: Option<&RetainedLayerPlan>,
    ) -> Result<ResolvedVerticalOverscanPlan, RetainedPaintError> {
        if viewport.x != 0.0 || viewport.y != 0.0 {
            return Err(RetainedPaintError::InvalidOverscanPlan);
        }
        let height = i64::from(dimensions.height);
        let first_y = i64::from(self.first_y);
        let last_index = i64::from(self.sample_count - 1);
        let last_y = first_y
            .checked_add(
                height
                    .checked_mul(last_index)
                    .ok_or(RetainedPaintError::InvalidOverscanPlan)?,
            )
            .ok_or(RetainedPaintError::InvalidOverscanPlan)?;
        let right = i64::from(self.document_x)
            .checked_add(i64::from(dimensions.width))
            .ok_or(RetainedPaintError::InvalidOverscanPlan)?;
        let bottom = last_y
            .checked_add(i64::from(dimensions.height))
            .ok_or(RetainedPaintError::InvalidOverscanPlan)?;
        if right > MAX_EXACT_F32_INTEGER || bottom > MAX_EXACT_F32_INTEGER {
            return Err(RetainedPaintError::InvalidOverscanPlan);
        }
        if expected_runs == 0 || expected_runs > MAX_RETAINED_RUNS || expected_runs % 2 == 0 {
            return Err(RetainedPaintError::OverscanRunMismatch);
        }
        let filter_runs = sources.map_or(0, FilterSourcePlan::active_count);
        let split_roots = expected_runs / 2;
        let fixed_runs = split_roots
            .checked_sub(filter_runs)
            .ok_or(RetainedPaintError::Capacity)?;
        let document_runs = split_roots + 1;
        let final_tiles = document_runs
            .checked_mul(usize::from(self.sample_count))
            .and_then(|tiles| tiles.checked_add(fixed_runs))
            .and_then(|tiles| tiles.checked_add(filter_runs * usize::from(self.sample_count)))
            .ok_or(RetainedPaintError::Capacity)?;
        let sample_y = (0..self.sample_count)
            .map(|index| {
                let y = first_y + height * i64::from(index);
                i32::try_from(y).map_err(|_| RetainedPaintError::InvalidOverscanPlan)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let document_origins = sample_y
            .iter()
            .map(|&y| (self.document_x, y))
            .collect::<Vec<_>>();
        let layer_budget = layers.map_or(Ok(RetainedLayerBudget::default()), |layers| {
            retained_layer_budget(layers, dimensions, &document_origins)
        })?;
        if layers
            .map(|layers| retained_layer_composite_taps(layers, dimensions, &document_origins))
            .transpose()?
            .is_some_and(|taps| taps > MAX_RETAINED_LAYER_COMPOSITE_TAPS)
        {
            return Err(RetainedPaintError::Capacity);
        }
        let flat_document_runs = document_runs
            .checked_sub(layer_budget.document_runs)
            .ok_or(RetainedPaintError::TraversalMismatch)?;
        let flat_fixed_runs = fixed_runs
            .checked_sub(layer_budget.fixed_runs)
            .ok_or(RetainedPaintError::TraversalMismatch)?;
        let full_tiles = flat_document_runs
            .checked_mul(usize::from(self.sample_count))
            .and_then(|tiles| tiles.checked_add(flat_fixed_runs))
            .ok_or(RetainedPaintError::Capacity)?;
        let layer_scratch = if layers.is_some() {
            let surfaces = if fixed_runs == 0 { 2 } else { 4 };
            dimensions
                .bytes
                .checked_mul(surfaces)
                .ok_or(RetainedPaintError::Capacity)?
        } else {
            0
        };
        let recorder_metadata = if layers.is_some() {
            recorder_metadata_peak_bytes()?
                .checked_mul(if fixed_runs == 0 { 1 } else { 2 })
                .ok_or(RetainedPaintError::Capacity)?
        } else {
            0
        };
        let scratch_bytes = sources
            .map_or(0, |sources| sources.scratch_bytes)
            .checked_add(layer_scratch)
            .ok_or(RetainedPaintError::Capacity)?;
        let metadata_bytes = sources
            .map_or(0, |sources| sources.metadata_bytes)
            .checked_add(layer_budget.metadata_bytes)
            .and_then(|bytes| bytes.checked_add(recorder_metadata))
            .ok_or(RetainedPaintError::Capacity)?;
        let peak_bytes = dimensions
            .bytes
            .checked_mul(full_tiles)
            .and_then(|bytes| bytes.checked_add(layer_budget.pixel_bytes))
            .and_then(|bytes| bytes.checked_add(sources.map_or(0, |sources| sources.source_bytes)))
            .and_then(|bytes| bytes.checked_add(scratch_bytes))
            .and_then(|bytes| bytes.checked_add(metadata_bytes))
            .ok_or(RetainedPaintError::Capacity)?;
        if final_tiles > MAX_RETAINED_TILES
            || !super::gradient_tiles::retained_peak_fits(peak_bytes, MAX_RETAINED_BYTES)
        {
            if let Some(sources) = sources {
                sources.diagnose_capacity("overscan", expected_runs, peak_bytes);
            }
            return Err(RetainedPaintError::Capacity);
        }
        Ok(ResolvedVerticalOverscanPlan {
            document_x: self.document_x,
            sample_y,
            current_index: usize::from(self.current_index),
            final_tiles,
            empty_filter_sources: sources.map_or_else(Vec::new, FilterSourcePlan::inactive),
            scratch_bytes,
            metadata_bytes,
        })
    }
}

/// Call-local view of the live paint inputs shared by all overscan samples.
/// It is never stored in the owned result or propagated into another owner.
#[derive(Clone, Copy)]
struct OverscanPaintView<'a> {
    raw_layout: &'a LayoutBox,
    viewport: Rect,
    fonts: &'a [Arc<Font>],
    web_fonts: Option<&'a WebFontRegistry>,
    visited_link_ids: &'a [usize],
    nested_documents: &'a HashMap<usize, Image>,
    shadow_masks: &'a border::FrameShadowMaskCache,
}

impl OverscanPaintView<'_> {
    fn expected_runs(
        self,
        resolver: &mut StyleResolver,
        dimensions: SurfaceDimensions,
        coverage: &ResolvedVerticalOverscanPlan,
    ) -> Result<OverscanPreflight, RetainedPaintError> {
        resolver.begin_visited_paint(self.visited_link_ids.iter().copied());
        let result = (|| {
            let sticky = StickyPlan::analyze(self.raw_layout, resolver, self.viewport, coverage)?;
            let filters = FilterSourcePlan::analyze(self, resolver, &sticky, coverage, dimensions)?;
            let plan = ExtractionPlan::analyze(
                self.raw_layout,
                resolver,
                dimensions,
                Some(InvariantOverflowClip::raw(self.viewport, coverage)),
                Some(&sticky),
                Some(&filters),
            )?;
            validate_vertical_overscan_layout(
                self.raw_layout,
                resolver,
                OverscanScanState::default(),
                &sticky,
                Some(&filters),
            )?;
            let current_scroll = (
                coverage.document_x,
                coverage.sample_y[coverage.current_index],
            );
            let mut current_layout = self.raw_layout.clone();
            apply_scroll_offsets(
                &mut current_layout,
                resolver,
                self.viewport,
                (current_scroll.0 as f32, current_scroll.1 as f32),
            );
            let current_filters = filters.at_scroll(current_scroll);
            let current_plan = ExtractionPlan::analyze(
                &current_layout,
                resolver,
                dimensions,
                Some(InvariantOverflowClip::adjusted(self.viewport)),
                sticky
                    .verifies_adjusted(&current_layout, current_scroll)
                    .then_some(&sticky),
                Some(&current_filters),
            )?;
            if current_plan.expected_runs != plan.expected_runs {
                return Err(RetainedPaintError::TraversalMismatch);
            }
            let planning = plan_retained_layers(
                &current_layout,
                resolver,
                self.viewport,
                self.fonts,
                self.web_fonts,
                Some(self.nested_documents),
                Some(self.shadow_masks),
                current_plan,
            )
            .map_err(|failure| failure.reason)?;
            let document_layers = Arc::new(planning.layers);
            Ok(OverscanPreflight {
                expected_runs: plan.expected_runs,
                sticky,
                filters,
                document_layers,
                current_frame: planning.current_frame.map(|frame| (planning.plan, frame)),
            })
        })();
        resolver.end_visited_paint();
        result
    }

    fn sample(
        self,
        resolver: &mut StyleResolver,
        scroll: (i32, i32),
        sticky: &StickyPlan,
        filters: &FilterSourcePlan,
        document_layers: &Arc<RetainedLayerPlan>,
    ) -> RetainedPaintOutcome {
        let mut layout = self.raw_layout.clone();
        apply_scroll_offsets(
            &mut layout,
            resolver,
            self.viewport,
            (scroll.0 as f32, scroll.1 as f32),
        );
        let filters = filters.at_scroll(scroll);
        paint_layout_with_retained_fallback_impl(
            &layout,
            resolver,
            self.viewport,
            self.fonts,
            self.web_fonts,
            Some(self.nested_documents),
            self.visited_link_ids,
            Some(self.shadow_masks),
            None,
            Some(InvariantOverflowClip::adjusted(self.viewport)),
            sticky.verifies_adjusted(&layout, scroll).then_some(sticky),
            Some(&filters),
            Some(Arc::clone(document_layers)),
        )
    }

    fn fallback(
        self,
        resolver: &mut StyleResolver,
        plan: &ResolvedVerticalOverscanPlan,
        reason: RetainedPaintError,
        discarded_tiles: usize,
    ) -> RetainedOverscanOutcome {
        let mut layout = self.raw_layout.clone();
        let scroll = (plan.document_x, plan.sample_y[plan.current_index]);
        apply_scroll_offsets(
            &mut layout,
            resolver,
            self.viewport,
            (scroll.0 as f32, scroll.1 as f32),
        );
        let (canvas, _) = paint_layout_with_document_snapshots_cached(
            &layout,
            resolver,
            self.viewport,
            self.fonts.to_vec(),
            self.web_fonts,
            self.visited_link_ids.iter().copied(),
            self.nested_documents,
            false,
            self.shadow_masks,
        );
        RetainedOverscanOutcome::Fallback {
            canvas,
            reason,
            discarded_tiles,
        }
    }
}

impl RetainedPaintFrame {
    /// Returns the extracted runs in exact painter traversal order.
    pub(crate) fn runs(&self) -> &[RetainedPaintRun] {
        &self.runs
    }

    /// Re-composites all runs using the ordinary canvas source-over path.
    pub(crate) fn compose(&self) -> Canvas {
        let mut canvas = Canvas::new(self.width, self.height);
        for run in &self.runs {
            if let Some(source) = &run.filter_source {
                source.compose_current(&mut canvas);
                continue;
            }
            canvas.draw_image(&run.image, 0.0, 0.0);
            for layer in &run.document_layers {
                if let Some(image) = &layer.image {
                    canvas.draw_image(&image.image, image.origin.0 as f32, image.origin.1 as f32);
                }
            }
        }
        canvas
    }
}

/// Conservative reason that extraction returned the ordinary painter result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RetainedPaintError {
    InvalidViewport,
    Capacity,
    StickyPosition,
    TopLayer,
    FixedBackground,
    BackdropFilter,
    CrossSpaceEffect,
    CrossSpaceClip,
    TraversalMismatch,
    CollectionFailed,
    InvalidOverscanPlan,
    OverscanRunMismatch,
    FixedRunChanged,
    ViewportEdgeFilter,
    OutsideOverscanCoverage,
    InvalidOverscanRasterScale,
    InvalidOverscanScroll,
    InvalidOverscanFrame,
    IncompleteOverscanCoverage,
    OverlappingOverscanTiles,
}

/// Result of bounded vertical overscan extraction.
pub(crate) enum RetainedOverscanOutcome {
    Extracted(RetainedOverscanFrame),
    Fallback {
        canvas: Canvas,
        reason: RetainedPaintError,
        discarded_tiles: usize,
    },
    Rejected {
        reason: RetainedPaintError,
    },
}

/// Either a fully owned retained frame or the ordinary painter fallback.
pub(crate) enum RetainedPaintOutcome {
    Extracted(RetainedPaintFrame),
    Fallback {
        canvas: Canvas,
        reason: RetainedPaintError,
        discarded_runs: usize,
    },
    /// The requested output surface was unsafe to allocate, so no paint was
    /// attempted and no fallback canvas exists.
    Rejected {
        reason: RetainedPaintError,
    },
}

impl RetainedPaintOutcome {
    /// Returns pixels when the requested surface passed the allocation guard.
    pub(crate) fn into_canvas(self) -> Result<Canvas, RetainedPaintError> {
        match self {
            Self::Extracted(frame) => Ok(frame.compose()),
            Self::Fallback { canvas, .. } => Ok(canvas),
            Self::Rejected { reason } => Err(reason),
        }
    }
}

/// Identifies whether the current canvas is a root output or a local surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PaintDestination {
    RootDocument,
    Temporary,
    FilterSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct LayoutAddress(usize);

impl LayoutAddress {
    fn of(layout: &LayoutBox) -> Self {
        // The immutable layout tree outlives the complete paint call. These
        // addresses are call-local and are never included in retained output.
        Self(layout as *const LayoutBox as usize)
    }
}

#[derive(Clone)]
struct ExtractionPlan {
    width: u32,
    height: u32,
    surface_bytes: usize,
    expected_runs: usize,
    fixed_roots: HashMap<LayoutAddress, usize>,
    filter_sources: Option<FilterSourcePlan>,
    document_layers: Option<Arc<RetainedLayerPlan>>,
    document_origin: (i32, i32),
}

#[derive(Clone, Copy)]
struct SurfaceDimensions {
    width: u32,
    height: u32,
    bytes: usize,
}

#[derive(Clone, Copy, Default)]
struct RetainedLayerBudget {
    pixel_bytes: usize,
    metadata_bytes: usize,
    sample_count: usize,
    document_runs: usize,
    fixed_runs: usize,
}

fn layer_coverage_rect(
    origin: (i32, i32),
    dimensions: SurfaceDimensions,
) -> Result<PixelRect, RetainedPaintError> {
    Ok(PixelRect {
        left: origin.0,
        top: origin.1,
        right: origin
            .0
            .checked_add(i32::try_from(dimensions.width).map_err(|_| RetainedPaintError::Capacity)?)
            .ok_or(RetainedPaintError::Capacity)?,
        bottom: origin
            .1
            .checked_add(
                i32::try_from(dimensions.height).map_err(|_| RetainedPaintError::Capacity)?,
            )
            .ok_or(RetainedPaintError::Capacity)?,
    })
}

fn retained_layer_budget(
    layers: &RetainedLayerPlan,
    dimensions: SurfaceDimensions,
    document_origins: &[(i32, i32)],
) -> Result<RetainedLayerBudget, RetainedPaintError> {
    let mut budget = RetainedLayerBudget {
        metadata_bytes: layers.metadata_bytes()?,
        ..RetainedLayerBudget::default()
    };
    for run in &layers.runs {
        let fixed_origin = [(0, 0)];
        let origins: &[(i32, i32)] = match run.identity {
            RetainedPaintRunIdentity::Document(_) => {
                budget.document_runs = budget
                    .document_runs
                    .checked_add(1)
                    .ok_or(RetainedPaintError::Capacity)?;
                document_origins
            }
            RetainedPaintRunIdentity::ViewportFixed(_) => {
                budget.fixed_runs = budget
                    .fixed_runs
                    .checked_add(1)
                    .ok_or(RetainedPaintError::Capacity)?;
                &fixed_origin
            }
            RetainedPaintRunIdentity::FilterSource(_) => {
                return Err(RetainedPaintError::TraversalMismatch);
            }
        };
        if run.base_first {
            if run.identity != RetainedPaintRunIdentity::Document(0) {
                return Err(RetainedPaintError::TraversalMismatch);
            }
            budget.pixel_bytes = dimensions
                .bytes
                .checked_mul(origins.len())
                .and_then(|bytes| budget.pixel_bytes.checked_add(bytes))
                .ok_or(RetainedPaintError::Capacity)?;
        }
        for layer in run.layers.iter() {
            for &origin in origins {
                let coverage = layer_coverage_rect(origin, dimensions)?;
                let pixels = layer
                    .sample_rect
                    .intersection(coverage)
                    .map_or(0, |rect| rect.bytes().unwrap_or(usize::MAX));
                let animation = layer
                    .animation_count
                    .checked_mul(std::mem::size_of::<animation::AnimationRegion>())
                    .ok_or(RetainedPaintError::Capacity)?;
                budget.pixel_bytes = budget
                    .pixel_bytes
                    .checked_add(pixels)
                    .and_then(|bytes| bytes.checked_add(animation))
                    .ok_or(RetainedPaintError::Capacity)?;
                budget.sample_count = budget
                    .sample_count
                    .checked_add(1)
                    .ok_or(RetainedPaintError::Capacity)?;
            }
        }
    }
    budget.metadata_bytes = budget
        .metadata_bytes
        .checked_add(
            budget
                .sample_count
                .checked_mul(std::mem::size_of::<RetainedLayerSample>())
                .ok_or(RetainedPaintError::Capacity)?,
        )
        .ok_or(RetainedPaintError::Capacity)?;
    if budget.sample_count > MAX_RETAINED_LAYER_SAMPLES {
        return Err(RetainedPaintError::Capacity);
    }
    Ok(budget)
}

fn retained_layer_composite_taps(
    layers: &RetainedLayerPlan,
    dimensions: SurfaceDimensions,
    document_origins: &[(i32, i32)],
) -> Result<usize, RetainedPaintError> {
    let viewport_pixels = (dimensions.width as usize)
        .checked_mul(dimensions.height as usize)
        .ok_or(RetainedPaintError::Capacity)?;
    let mut taps = 0usize;
    for run in &layers.runs {
        let plane_count = run
            .layers
            .len()
            .checked_add(usize::from(run.base_first))
            .ok_or(RetainedPaintError::Capacity)?;
        if !matches!(run.identity, RetainedPaintRunIdentity::Document(_)) || plane_count < 2 {
            continue;
        }
        taps = taps
            .checked_add(
                viewport_pixels
                    .checked_mul(4)
                    .ok_or(RetainedPaintError::Capacity)?,
            )
            .ok_or(RetainedPaintError::Capacity)?;
        taps = taps
            .checked_add(
                viewport_pixels
                    .checked_mul(4)
                    .and_then(|value| value.checked_mul(run.layers.len()))
                    .ok_or(RetainedPaintError::Capacity)?,
            )
            .ok_or(RetainedPaintError::Capacity)?;
        for layer in run.layers.iter() {
            for &origin in document_origins {
                let coverage = layer_coverage_rect(origin, dimensions)?;
                let pixels = layer
                    .sample_rect
                    .intersection(coverage)
                    .map_or(0, |rect| rect.bytes().unwrap_or(usize::MAX) / 4);
                taps = taps
                    .checked_add(pixels.checked_mul(4).ok_or(RetainedPaintError::Capacity)?)
                    .ok_or(RetainedPaintError::Capacity)?;
            }
        }
    }
    Ok(taps)
}

impl SurfaceDimensions {
    fn for_viewport(viewport: Rect) -> Result<Self, RetainedPaintError> {
        if !viewport.x.is_finite()
            || !viewport.y.is_finite()
            || !viewport.width.is_finite()
            || !viewport.height.is_finite()
            || viewport.width <= 0.0
            || viewport.height <= 0.0
        {
            return Err(RetainedPaintError::InvalidViewport);
        }
        let width = viewport.width.ceil() as f64;
        let height = viewport.height.ceil() as f64;
        if width > u32::MAX as f64 || height > u32::MAX as f64 {
            return Err(RetainedPaintError::Capacity);
        }
        let width = width as u32;
        let height = height as u32;
        let bytes = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or(RetainedPaintError::Capacity)?;
        if !super::gradient_tiles::retained_peak_fits(bytes, MAX_RETAINED_BYTES) {
            return Err(RetainedPaintError::Capacity);
        }
        Ok(Self {
            width,
            height,
            bytes,
        })
    }
}

/// Overscan-only proof domain. Every ancestor clip must contain the viewport
/// for the complete scroll interval, not merely at the captured sample offsets.
/// The ordinary single-frame extractor never receives this permission.
#[derive(Clone, Copy)]
struct InvariantOverflowClip {
    viewport: Rect,
    minimum: (f32, f32),
    maximum: (f32, f32),
}

impl InvariantOverflowClip {
    fn raw(viewport: Rect, plan: &ResolvedVerticalOverscanPlan) -> Self {
        Self {
            viewport,
            minimum: (plan.document_x as f32, plan.sample_y[0] as f32),
            maximum: (
                plan.document_x as f32,
                *plan.sample_y.last().expect("validated nonempty samples") as f32,
            ),
        }
    }

    fn adjusted(viewport: Rect) -> Self {
        Self {
            viewport,
            minimum: (0.0, 0.0),
            maximum: (0.0, 0.0),
        }
    }

    fn covers_viewport(self, layout: &LayoutBox, style: &ComputedStyle) -> bool {
        if crate::layout::has_containment(style, "paint")
            || has_non_none_value(style, "clip-path")
            || [
                "border-top-left-radius",
                "border-top-right-radius",
                "border-bottom-right-radius",
                "border-bottom-left-radius",
            ]
            .into_iter()
            .any(|name| {
                style.get(name).is_some_and(|value| {
                    !matches!(value, ComputedValue::Number(number) if *number == 0.0)
                        && value.linear_length_percentage_components() != Some((0.0, 0.0))
                })
            })
        {
            return false;
        }
        let padding_at = |scroll: (f32, f32)| {
            // Match apply_scroll_offsets then padding_box_rect, including f32
            // operation order. Each edge is monotonic in the scroll offset.
            let mut content = layout.dimensions.content;
            content.x += -scroll.0;
            content.y += -scroll.1;
            let padding = layout.dimensions.padding;
            Rect {
                x: content.x - padding.left,
                y: content.y - padding.top,
                width: content.width + padding.left + padding.right,
                height: content.height + padding.top + padding.bottom,
            }
        };
        let first = padding_at(self.minimum);
        let last = padding_at(self.maximum);
        let finite = |rect: Rect| {
            [
                rect.x,
                rect.y,
                rect.width,
                rect.height,
                rect.x + rect.width,
                rect.y + rect.height,
            ]
            .into_iter()
            .all(f32::is_finite)
                && rect.width >= 0.0
                && rect.height >= 0.0
        };
        if !finite(first) || !finite(last) {
            return false;
        }
        // Left/top attain their largest values at minimum scroll; right/bottom
        // attain their smallest at maximum scroll. These inequalities therefore
        // prove containment for ALL intermediate (also fractional) offsets.
        (!layout.overflow.clips_x()
            || (first.x <= self.viewport.x
                && last.x + last.width >= self.viewport.x + self.viewport.width))
            && (!layout.overflow.clips_y()
                || (first.y <= self.viewport.y
                    && last.y + last.height >= self.viewport.y + self.viewport.height))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StickySpace {
    Document,
    Viewport,
}

#[derive(Clone, Copy)]
struct StickyEntry {
    space: StickySpace,
    content: Rect,
}

#[derive(Default)]
struct StickyPlan {
    entries: HashMap<usize, StickyEntry>,
}

#[derive(Clone, Copy)]
struct StickyCoverage {
    viewport: Rect,
    minimum_y: f32,
    maximum_y: f32,
}

#[derive(Clone, Copy)]
enum StickyScrollport {
    Viewport,
    Document {
        content: Rect,
        padding_top: f32,
        height: f32,
    },
}

#[derive(Clone, Copy)]
struct StickyContext {
    parent: Option<Rect>,
    scrollport: StickyScrollport,
    unsupported: bool,
    nested: bool,
    depth: u16,
}

impl StickyPlan {
    fn analyze(
        layout: &LayoutBox,
        resolver: &mut StyleResolver,
        viewport: Rect,
        plan: &ResolvedVerticalOverscanPlan,
    ) -> Result<Self, RetainedPaintError> {
        let coverage = StickyCoverage {
            viewport,
            minimum_y: plan.sample_y[0] as f32,
            maximum_y: *plan.sample_y.last().expect("validated samples") as f32,
        };
        let mut result = Self::default();
        scan_sticky_proofs(
            layout,
            resolver,
            coverage,
            StickyContext {
                parent: None,
                scrollport: StickyScrollport::Viewport,
                unsupported: false,
                nested: false,
                depth: 0,
            },
            &mut result,
        )?;
        Ok(result)
    }

    fn space(&self, layout: &LayoutBox) -> Option<StickySpace> {
        self.entries
            .get(&layout.node.identity())
            .map(|entry| entry.space)
    }

    fn verifies_adjusted(&self, layout: &LayoutBox, scroll: (i32, i32)) -> bool {
        let mut seen = HashSet::new();
        self.verify_node(layout, scroll, &mut seen) && seen.len() == self.entries.len()
    }

    fn verify_node(
        &self,
        layout: &LayoutBox,
        scroll: (i32, i32),
        seen: &mut HashSet<usize>,
    ) -> bool {
        let id = layout.node.identity();
        if let Some(entry) = self.entries.get(&id) {
            let mut expected = entry.content;
            expected.x += -(scroll.0 as f32);
            if entry.space == StickySpace::Document {
                expected.y += -(scroll.1 as f32);
            }
            if !seen.insert(id) || layout.dimensions.content != expected {
                return false;
            }
        }
        layout
            .children
            .iter()
            .all(|child| self.verify_node(child, scroll, seen))
    }
}

fn scan_sticky_proofs(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    coverage: StickyCoverage,
    context: StickyContext,
    result: &mut StickyPlan,
) -> Result<(), RetainedPaintError> {
    if layout.visibility == Visibility::Hidden {
        return Ok(());
    }
    let style = paint_box_style(layout, resolver);
    if context.depth >= 128 {
        trace_unsupported_sticky(layout, &style, coverage, context, true);
        return Err(RetainedPaintError::StickyPosition);
    }
    let sticky = is_sticky_for_paint(&style);
    let unsupported = context.unsupported
        || !layout.block_fragments.is_empty()
        || layout.transform != AffineTransform::identity()
        || has_transform_style(&style)
        || layout.node.scroll_offset() != (0.0, 0.0)
        || is_fixed_for_paint(&style)
        || crate::layout::establishes_fixed_containing_block(&style);
    if sticky {
        let entry = (!unsupported && !context.nested)
            .then(|| classify_sticky(layout, &style, coverage, context))
            .flatten();
        let Some(entry) = entry else {
            trace_unsupported_sticky(layout, &style, coverage, context, unsupported);
            return Err(RetainedPaintError::StickyPosition);
        };
        if result.entries.len() >= MAX_VIEWPORT_FIXED_ROOTS
            || result
                .entries
                .insert(layout.node.identity(), entry)
                .is_some()
        {
            return Err(RetainedPaintError::StickyPosition);
        }
    }
    if layout.content_visibility_contents_skipped {
        return Ok(());
    }
    let scrollport = if layout.is_scroll_container() {
        StickyScrollport::Document {
            content: layout.dimensions.content,
            padding_top: layout.dimensions.padding.top,
            height: padding_box_rect(layout).height,
        }
    } else {
        context.scrollport
    };
    let child_context = StickyContext {
        parent: Some(layout.dimensions.content),
        scrollport,
        unsupported,
        nested: context.nested || sticky,
        depth: context.depth + 1,
    };
    for child in &layout.children {
        scan_sticky_proofs(child, resolver, coverage, child_context, result)?;
    }
    Ok(())
}

fn classify_sticky(
    layout: &LayoutBox,
    style: &ComputedStyle,
    coverage: StickyCoverage,
    context: StickyContext,
) -> Option<StickyEntry> {
    let parent = context.parent?;
    // Horizontal scroll is constant across this vertical coverage, but keep
    // the proof and adjusted-coordinate check on the exact uncorrected path.
    if sticky_inset(style, "left", parent.width).is_some()
        || sticky_inset(style, "right", parent.width).is_some()
    {
        return None;
    }
    let border = border_box_rect(layout);
    let margin = layout.dimensions.margin;
    let start = border.y - margin.top;
    let size = border.height + margin.top + margin.bottom;
    let top = sticky_inset(style, "top", parent.height);
    let bottom = sticky_inset(style, "bottom", parent.height);
    if ![start, size, parent.y, parent.height, coverage.maximum_y]
        .into_iter()
        .all(f32::is_finite)
        || size < 0.0
        || parent.height < 0.0
        || top.is_some_and(|value| !value.is_finite())
        || bottom.is_some_and(|value| !value.is_finite())
    {
        return None;
    }
    if sticky_axis_stays_zero(
        start,
        size,
        parent,
        top,
        bottom,
        coverage,
        context.scrollport,
    ) {
        return Some(StickyEntry {
            space: StickySpace::Document,
            content: layout.dimensions.content,
        });
    }
    if matches!(context.scrollport, StickyScrollport::Viewport)
        && coverage.viewport.y == 0.0
        && let Some(offset) =
            viewport_sticky_linear_offset(start, size, parent, top, bottom, coverage)
    {
        let mut content = layout.dimensions.content;
        content.y += offset;
        return Some(StickyEntry {
            space: StickySpace::Viewport,
            content,
        });
    }
    None
}

/// Proves that the viewport sticky translation is `scroll + offset` over the
/// complete retained range. The adjusted box position is therefore constant
/// and may be extracted into a viewport-fixed paint run.
fn viewport_sticky_linear_offset(
    start: f32,
    size: f32,
    parent: Rect,
    top: Option<f32>,
    bottom: Option<f32>,
    coverage: StickyCoverage,
) -> Option<f32> {
    let top = top.unwrap_or(0.0);
    let mut bottom = bottom.unwrap_or(0.0);
    let view_size = coverage.viewport.height - top - bottom;
    if view_size < size {
        bottom -= size - view_size;
    }
    let lower_offset = coverage.viewport.y + top - start;
    let upper_offset = coverage.viewport.y + coverage.viewport.height - bottom - start - size;
    let low = lower_offset.min(upper_offset);
    let high = lower_offset.max(upper_offset);
    let offset = if coverage.minimum_y + low >= 0.0 {
        low
    } else if coverage.maximum_y + high <= 0.0 {
        high
    } else {
        return None;
    };
    let containing_low = (parent.y - start).min(parent.y + parent.height - start - size);
    let containing_high = (parent.y - start).max(parent.y + parent.height - start - size);
    let desired_min = coverage.minimum_y + offset;
    let desired_max = coverage.maximum_y + offset;
    let proof_values = [
        start,
        size,
        parent.y,
        parent.height,
        coverage.viewport.height,
        coverage.minimum_y,
        coverage.maximum_y,
        offset,
        containing_low,
        containing_high,
        desired_min,
        desired_max,
    ];
    proof_values
        .into_iter()
        .all(|value| value.is_finite() && value.abs() <= 1_048_576.0)
        .then_some(())?;
    (desired_min >= containing_low && desired_max <= containing_high).then_some(offset)
}

#[derive(Clone, Copy)]
struct StickyRange {
    minimum: f32,
    maximum: f32,
}

impl StickyRange {
    fn constant(value: f32) -> Self {
        Self {
            minimum: value,
            maximum: value,
        }
    }

    fn plus(self, other: Self) -> Self {
        Self {
            minimum: self.minimum + other.minimum,
            maximum: self.maximum + other.maximum,
        }
    }

    fn minus(self, other: Self) -> Self {
        Self {
            minimum: self.minimum - other.maximum,
            maximum: self.maximum - other.minimum,
        }
    }

    fn finite(self) -> bool {
        self.minimum.is_finite() && self.maximum.is_finite()
    }
}

#[allow(clippy::too_many_arguments)]
fn sticky_axis_stays_zero(
    start: f32,
    size: f32,
    parent: Rect,
    top: Option<f32>,
    bottom: Option<f32>,
    coverage: StickyCoverage,
    scrollport: StickyScrollport,
) -> bool {
    if top.is_none() && bottom.is_none() {
        return true;
    }
    let (port_start, port_size) = match scrollport {
        StickyScrollport::Viewport => (
            StickyRange::constant(coverage.viewport.y),
            coverage.viewport.height,
        ),
        StickyScrollport::Document {
            content,
            padding_top,
            height,
        } => {
            // Shared -scroll terms cancel exactly in this common zero-origin
            // body-scrollport case. Monotone addition proves both upper bounds
            // nonnegative even with f32 rounding, so both clamps return 0.
            if start == 0.0
                && parent.y == 0.0
                && content.y == 0.0
                && padding_top == 0.0
                && top == Some(0.0)
                && bottom.is_none()
                && height.is_finite()
                && size <= height
                && size <= parent.height
            {
                return true;
            }
            (
                StickyRange::constant(content.y)
                    .plus(StickyRange {
                        minimum: -coverage.maximum_y,
                        maximum: -coverage.minimum_y,
                    })
                    .minus(StickyRange::constant(padding_top)),
                height,
            )
        }
    };
    let top = top.unwrap_or(0.0);
    let mut bottom = bottom.unwrap_or(0.0);
    let view_size = port_size - top - bottom;
    if view_size < size {
        bottom -= size - view_size;
    }
    let delta = StickyRange {
        minimum: -coverage.maximum_y,
        maximum: -coverage.minimum_y,
    };
    let box_start = StickyRange::constant(start).plus(delta);
    let box_end = box_start.plus(StickyRange::constant(size));
    let lower = port_start.plus(StickyRange::constant(top)).minus(box_start);
    let upper = port_start
        .plus(StickyRange::constant(port_size))
        .minus(StickyRange::constant(bottom))
        .minus(box_end);
    let containing_start = StickyRange::constant(parent.y).plus(delta);
    let containing_lower = containing_start.minus(box_start);
    let containing_upper = containing_start
        .plus(StickyRange::constant(parent.height))
        .minus(box_end);
    let finite = [lower, upper, containing_lower, containing_upper]
        .into_iter()
        .all(StickyRange::finite);
    // The containing clamp is exactly [0,0] when its two f32 expressions
    // have identical inputs. It returns zero regardless of the desired phase.
    let pinned_by_containing_block = start == parent.y && size == parent.height;
    finite
        && (pinned_by_containing_block
            || (lower.maximum <= 0.0
                && upper.minimum >= 0.0
                && containing_lower.maximum <= 0.0
                && containing_upper.minimum >= 0.0))
}

fn trace_unsupported_sticky(
    layout: &LayoutBox,
    style: &ComputedStyle,
    coverage: StickyCoverage,
    context: StickyContext,
    unsupported: bool,
) {
    if std::env::var_os("OMOIKANE_TRACE_RETAINED_STICKY").is_none() {
        return;
    }
    let rect = |rect: Rect| [rect.x, rect.y, rect.width, rect.height];
    let parent = context.parent;
    let (port_content, port_padding_top, port_height) = match context.scrollport {
        StickyScrollport::Viewport => (None, 0.0, coverage.viewport.height),
        StickyScrollport::Document {
            content,
            padding_top,
            height,
        } => (Some(rect(content)), padding_top, height),
    };
    eprintln!(
        "OMOIKANE_RETAINED_STICKY {}",
        serde_json::json!({
            "node_id": layout.node.identity(),
            "tag": layout.node.tag_name().map(|tag| tag.chars().take(48).collect::<String>()),
            "raw_content": rect(layout.dimensions.content),
            "raw_margin": rect({
                let border = border_box_rect(layout);
                let margin = layout.dimensions.margin;
                Rect { x: border.x - margin.left, y: border.y - margin.top,
                    width: border.width + margin.left + margin.right,
                    height: border.height + margin.top + margin.bottom }
            }),
            "parent_content": parent.map(rect),
            "viewport": rect(coverage.viewport),
            "coverage_y": [coverage.minimum_y, coverage.maximum_y],
            "top": parent.and_then(|p| sticky_inset(style, "top", p.height)),
            "bottom": parent.and_then(|p| sticky_inset(style, "bottom", p.height)),
            "left": parent.and_then(|p| sticky_inset(style, "left", p.width)),
            "right": parent.and_then(|p| sticky_inset(style, "right", p.width)),
            "nested": context.nested,
            "depth": context.depth,
            "scrollport_content": port_content,
            "scrollport_padding_top": port_padding_top,
            "scrollport_height": port_height,
            "unsupported_ancestor_or_transform": unsupported,
            "document_scrollport": matches!(context.scrollport, StickyScrollport::Document { .. }),
        })
    );
}

struct OverscanPreflight {
    expected_runs: usize,
    sticky: StickyPlan,
    filters: FilterSourcePlan,
    document_layers: Arc<RetainedLayerPlan>,
    current_frame: Option<(ExtractionPlan, RetainedPaintFrame)>,
}

#[derive(Clone, Copy, Default)]
struct ScanState {
    fixed_containing_ancestor: bool,
    inside_viewport_fixed: bool,
    effect_ancestor: bool,
    clip_ancestor: bool,
    invariant_clip_ancestor: bool,
    clip_geometry_unproven: bool,
}

impl ExtractionPlan {
    fn ensure_layer_planning_capacity(&self) -> Result<(), RetainedPaintError> {
        let filters = self.filter_sources.as_ref();
        let non_filter_runs = self
            .expected_runs
            .checked_sub(filters.map_or(0, FilterSourcePlan::active_count))
            .ok_or(RetainedPaintError::Capacity)?;
        // At the last planned run, all earlier run images, the public Canvas,
        // and the recorder scratch surface can be alive together.
        let transient_surfaces = if self.fixed_roots.is_empty() { 2 } else { 4 };
        let surface_peak = non_filter_runs
            .checked_add(transient_surfaces)
            .and_then(|surfaces| surfaces.checked_mul(self.surface_bytes))
            .ok_or(RetainedPaintError::Capacity)?;
        let recorder_metadata = recorder_metadata_peak_bytes()?
            .checked_mul(if self.fixed_roots.is_empty() { 1 } else { 2 })
            .ok_or(RetainedPaintError::Capacity)?;
        let plan_metadata = layer_plan_metadata_max_bytes(non_filter_runs)?;
        let peak = surface_peak
            .checked_add(filters.map_or(0, |plan| plan.source_bytes))
            .and_then(|bytes| bytes.checked_add(filters.map_or(0, |plan| plan.scratch_bytes)))
            .and_then(|bytes| bytes.checked_add(filters.map_or(0, |plan| plan.metadata_bytes)))
            .and_then(|bytes| bytes.checked_add(recorder_metadata))
            .and_then(|bytes| bytes.checked_add(plan_metadata))
            .ok_or(RetainedPaintError::Capacity)?;
        if !super::gradient_tiles::retained_peak_fits(peak, MAX_RETAINED_BYTES) {
            return Err(RetainedPaintError::Capacity);
        }
        Ok(())
    }

    fn analyze(
        layout: &LayoutBox,
        resolver: &mut StyleResolver,
        dimensions: SurfaceDimensions,
        invariant_clip: Option<InvariantOverflowClip>,
        sticky: Option<&StickyPlan>,
        filter_sources: Option<&FilterSourcePlan>,
    ) -> Result<Self, RetainedPaintError> {
        let mut fixed_roots = HashMap::new();
        scan_layout(
            layout,
            resolver,
            ScanState::default(),
            &mut fixed_roots,
            invariant_clip,
            sticky,
            filter_sources,
        )?;
        let expected_runs = fixed_roots
            .len()
            .checked_add(filter_sources.map_or(0, FilterSourcePlan::active_count))
            .ok_or(RetainedPaintError::Capacity)?
            .checked_mul(2)
            .and_then(|runs| runs.checked_add(1))
            .ok_or(RetainedPaintError::Capacity)?;
        let retained_bytes = dimensions
            .bytes
            .checked_mul(expected_runs - filter_sources.map_or(0, FilterSourcePlan::active_count))
            .and_then(|bytes| bytes.checked_add(filter_sources.map_or(0, |plan| plan.source_bytes)))
            .and_then(|bytes| {
                bytes.checked_add(filter_sources.map_or(0, |plan| plan.scratch_bytes))
            })
            .and_then(|bytes| {
                bytes.checked_add(filter_sources.map_or(0, |plan| plan.metadata_bytes))
            })
            .ok_or(RetainedPaintError::Capacity)?;
        if expected_runs > MAX_RETAINED_RUNS
            || !super::gradient_tiles::retained_peak_fits(retained_bytes, MAX_RETAINED_BYTES)
        {
            if let Some(sources) = filter_sources {
                sources.diagnose_capacity("capture", expected_runs, retained_bytes);
            }
            return Err(RetainedPaintError::Capacity);
        }
        Ok(Self {
            width: dimensions.width,
            height: dimensions.height,
            surface_bytes: dimensions.bytes,
            expected_runs,
            fixed_roots,
            filter_sources: filter_sources.cloned(),
            document_layers: None,
            document_origin: filter_sources.map_or((0, 0), |plan| plan.scroll),
        })
    }

    fn install_document_layers(
        &mut self,
        layers: Arc<RetainedLayerPlan>,
    ) -> Result<(), RetainedPaintError> {
        if self.document_layers.is_some() {
            return Err(RetainedPaintError::TraversalMismatch);
        }
        let dimensions = SurfaceDimensions {
            width: self.width,
            height: self.height,
            bytes: self.surface_bytes,
        };
        let budget = retained_layer_budget(&layers, dimensions, &[self.document_origin])?;
        let filter_runs = self
            .filter_sources
            .as_ref()
            .map_or(0, FilterSourcePlan::active_count);
        let flat_runs = self
            .expected_runs
            .checked_sub(filter_runs)
            .and_then(|runs| {
                budget
                    .document_runs
                    .checked_add(budget.fixed_runs)
                    .and_then(|layer_runs| runs.checked_sub(layer_runs))
            })
            .ok_or_else(|| {
                trace_layer_mismatch("install_run_counts", || {
                    format!(
                        "expected={} filters={} planned_document={} planned_fixed={}",
                        self.expected_runs, filter_runs, budget.document_runs, budget.fixed_runs
                    )
                });
                RetainedPaintError::TraversalMismatch
            })?;
        let filters = self.filter_sources.as_ref();
        let recorder_metadata = recorder_metadata_peak_bytes()?
            .checked_mul(if self.fixed_roots.is_empty() { 1 } else { 2 })
            .ok_or(RetainedPaintError::Capacity)?;
        let transient_surfaces = if self.fixed_roots.is_empty() { 2 } else { 4 };
        let retained_bytes = flat_runs
            .checked_mul(self.surface_bytes)
            .and_then(|bytes| bytes.checked_add(budget.pixel_bytes))
            .and_then(|bytes| bytes.checked_add(budget.metadata_bytes))
            .and_then(|bytes| bytes.checked_add(recorder_metadata))
            .and_then(|bytes| {
                self.surface_bytes
                    .checked_mul(transient_surfaces)
                    .and_then(|transient| bytes.checked_add(transient))
            })
            .and_then(|bytes| bytes.checked_add(filters.map_or(0, |plan| plan.source_bytes)))
            .and_then(|bytes| bytes.checked_add(filters.map_or(0, |plan| plan.scratch_bytes)))
            .and_then(|bytes| bytes.checked_add(filters.map_or(0, |plan| plan.metadata_bytes)))
            .ok_or(RetainedPaintError::Capacity)?;
        if !super::gradient_tiles::retained_peak_fits(retained_bytes, MAX_RETAINED_BYTES) {
            return Err(RetainedPaintError::Capacity);
        }
        self.document_layers = Some(layers);
        Ok(())
    }
}

fn scan_layout(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    state: ScanState,
    fixed_roots: &mut HashMap<LayoutAddress, usize>,
    invariant_clip: Option<InvariantOverflowClip>,
    sticky: Option<&StickyPlan>,
    filter_sources: Option<&FilterSourcePlan>,
) -> Result<(), RetainedPaintError> {
    // A top-layer backdrop is painted before visibility is checked.
    if layout.node.top_layer_order().is_some() {
        return Err(RetainedPaintError::TopLayer);
    }
    if layout.visibility == Visibility::Hidden {
        return Ok(());
    }

    let style = paint_box_style(layout, resolver);
    let sticky_space = if is_sticky_for_paint(&style) {
        Some(
            sticky
                .and_then(|plan| plan.space(layout))
                .ok_or(RetainedPaintError::StickyPosition)?,
        )
    } else {
        None
    };
    if has_fixed_background(&style) {
        return Err(RetainedPaintError::FixedBackground);
    }
    if has_non_none_value(&style, "backdrop-filter") {
        return Err(RetainedPaintError::BackdropFilter);
    }

    let viewport_fixed = (is_fixed_for_paint(&style)
        || sticky_space == Some(StickySpace::Viewport))
        && !state.fixed_containing_ancestor
        && !state.inside_viewport_fixed;
    if viewport_fixed && state.effect_ancestor {
        return Err(RetainedPaintError::CrossSpaceEffect);
    }
    if viewport_fixed
        && (state.clip_ancestor || (state.invariant_clip_ancestor && state.clip_geometry_unproven))
    {
        return Err(RetainedPaintError::CrossSpaceClip);
    }
    if viewport_fixed {
        if fixed_roots.len() >= MAX_VIEWPORT_FIXED_ROOTS {
            return Err(RetainedPaintError::Capacity);
        }
        let next_id = fixed_roots.len();
        fixed_roots.insert(LayoutAddress::of(layout), next_id);
    }
    if layout.content_visibility_contents_skipped {
        return Ok(());
    }
    scan_inline_fragment_styles(layout, resolver)?;

    let child_state = child_scan_state(layout, &style, state, viewport_fixed, invariant_clip);
    let atomic_selection =
        AtomicInlineSelection::build(layout).map_err(|()| RetainedPaintError::TraversalMismatch)?;
    for child in &layout.children {
        if atomic_selection.classify(child) == AtomicInlineChildPaint::Skipped {
            continue;
        }
        scan_layout(
            child,
            resolver,
            child_state,
            fixed_roots,
            invariant_clip,
            sticky,
            filter_sources,
        )?;
    }
    Ok(())
}

fn child_scan_state(
    layout: &LayoutBox,
    style: &ComputedStyle,
    state: ScanState,
    viewport_fixed: bool,
    invariant_clip: Option<InvariantOverflowClip>,
) -> ScanState {
    let grouping_effect = element_opacity(style).is_some_and(|value| value < 1.0)
        || has_non_none_value(style, "filter")
        || has_non_none_layer(style, "mask-image");
    let clip_geometry_unproven = invariant_clip.is_some()
        && (state.clip_geometry_unproven
            || layout.transform != AffineTransform::identity()
            || has_transform_style(style)
            || layout.node.scroll_offset() != (0.0, 0.0)
            || !layout.block_fragments.is_empty());
    let overflow_is_invariant = layout.overflow.clips_overflow()
        && !clip_geometry_unproven
        && invariant_clip.is_some_and(|proof| proof.covers_viewport(layout, style));
    let descendant_clip = (layout.overflow.clips_overflow() && !overflow_is_invariant)
        || crate::layout::has_containment(style, "paint")
        || has_non_none_value(style, "clip-path");
    ScanState {
        fixed_containing_ancestor: state.fixed_containing_ancestor
            || crate::layout::establishes_fixed_containing_block(style),
        inside_viewport_fixed: state.inside_viewport_fixed || viewport_fixed,
        effect_ancestor: if state.inside_viewport_fixed || viewport_fixed {
            false
        } else {
            state.effect_ancestor || grouping_effect
        },
        invariant_clip_ancestor: state.invariant_clip_ancestor
            || (layout.overflow.clips_overflow() && overflow_is_invariant),
        clip_geometry_unproven,
        clip_ancestor: if state.inside_viewport_fixed || viewport_fixed {
            false
        } else {
            state.clip_ancestor || descendant_clip
        },
    }
}

fn scan_inline_fragment_styles(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
) -> Result<(), RetainedPaintError> {
    for line in &layout.lines {
        for fragment in selected_line_fragments(line) {
            if fragment.style.visibility == Visibility::Hidden {
                continue;
            }
            let captured = match &fragment.content {
                InlineFragmentContent::Image(_, style)
                | InlineFragmentContent::InlineBox(style)
                | InlineFragmentContent::GeneratedBox(style)
                | InlineFragmentContent::FormControl(style, ..) => Some(style),
                _ => None,
            };
            let Some(captured) = captured else {
                continue;
            };
            let paint = text::fragment_box_paint_style(fragment, resolver);
            if has_fixed_background(paint.as_deref().unwrap_or(captured)) {
                return Err(RetainedPaintError::FixedBackground);
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Default)]
struct OverscanScanState {
    fixed_containing_ancestor: bool,
    inside_viewport_fixed: bool,
}

/// Per-sample effect surfaces are clipped to the viewport canvas. A document
/// filter kernel can therefore observe a transparent sample edge that an
/// ordinary paint at an intermediate scroll would not have. Viewport-fixed
/// subtrees keep one current-sample image and do not cross document tiles.
fn validate_vertical_overscan_layout(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    state: OverscanScanState,
    sticky: &StickyPlan,
    filter_sources: Option<&FilterSourcePlan>,
) -> Result<(), RetainedPaintError> {
    if layout.visibility == Visibility::Hidden {
        return Ok(());
    }
    let style = paint_box_style(layout, resolver);
    let viewport_fixed = (is_fixed_for_paint(&style)
        || sticky.space(layout) == Some(StickySpace::Viewport))
        && !state.fixed_containing_ancestor
        && !state.inside_viewport_fixed;
    let inside_viewport_fixed = state.inside_viewport_fixed || viewport_fixed;
    if !inside_viewport_fixed
        && has_viewport_edge_filter(&style)
        && !filter_sources.is_some_and(|plan| plan.permits(layout))
    {
        diagnostics::trace_viewport_edge_filter(layout, &style, false);
        return Err(RetainedPaintError::ViewportEdgeFilter);
    }
    if layout.content_visibility_contents_skipped {
        return Ok(());
    }
    if !inside_viewport_fixed && inline_fragments_have_viewport_edge_filter(layout, resolver) {
        return Err(RetainedPaintError::ViewportEdgeFilter);
    }
    let child_state = OverscanScanState {
        fixed_containing_ancestor: state.fixed_containing_ancestor
            || crate::layout::establishes_fixed_containing_block(&style),
        inside_viewport_fixed,
    };
    let atomic_selection =
        AtomicInlineSelection::build(layout).map_err(|()| RetainedPaintError::TraversalMismatch)?;
    for child in &layout.children {
        if atomic_selection.classify(child) == AtomicInlineChildPaint::Skipped {
            continue;
        }
        validate_vertical_overscan_layout(child, resolver, child_state, sticky, filter_sources)?;
    }
    Ok(())
}

/// Color and alpha filters depend only on their own source pixel, so an
/// adjacent viewport tile cannot change their output. Spatial kernels and
/// unknown computed values retain the conservative viewport-edge fallback.
fn has_viewport_edge_filter(style: &ComputedStyle) -> bool {
    if !has_non_none_value(style, "filter") {
        return false;
    }
    let Some(ComputedValue::Keyword(value) | ComputedValue::String(value)) = style.get("filter")
    else {
        return true;
    };
    let Some(filters) = crate::css::parse_filter_list(value) else {
        return true;
    };
    filters.iter().any(|filter| {
        matches!(
            filter,
            crate::css::FilterFunction::Blur(_) | crate::css::FilterFunction::DropShadow { .. }
        )
    })
}

fn inline_fragments_have_viewport_edge_filter(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
) -> bool {
    layout.lines.iter().any(|line| {
        selected_line_fragments(line).iter().any(|fragment| {
            if fragment.style.visibility == Visibility::Hidden {
                return false;
            }
            let captured = match &fragment.content {
                InlineFragmentContent::Image(_, style)
                | InlineFragmentContent::InlineBox(style)
                | InlineFragmentContent::GeneratedBox(style)
                | InlineFragmentContent::FormControl(style, ..) => Some(style),
                _ => None,
            };
            captured.is_some_and(|captured| {
                let paint = text::fragment_box_paint_style(fragment, resolver);
                let style = paint.as_deref().unwrap_or(captured);
                let rejected = has_viewport_edge_filter(style);
                if rejected {
                    diagnostics::trace_viewport_edge_filter(layout, style, true);
                }
                rejected
            })
        })
    })
}

fn has_non_none_value(style: &ComputedStyle, property: &str) -> bool {
    match style.get(property) {
        None => false,
        Some(ComputedValue::Keyword(value) | ComputedValue::String(value)) => {
            let value = value.trim();
            !value.is_empty() && !value.eq_ignore_ascii_case("none")
        }
        Some(_) => true,
    }
}

fn has_non_none_layer(style: &ComputedStyle, property: &str) -> bool {
    match style.get(property) {
        None => false,
        Some(ComputedValue::Keyword(value) | ComputedValue::String(value)) => {
            crate::css::split_top_level_commas(value)
                .into_iter()
                .map(str::trim)
                .any(|value| !value.is_empty() && !value.eq_ignore_ascii_case("none"))
        }
        Some(_) => true,
    }
}

fn has_fixed_background(style: &ComputedStyle) -> bool {
    style
        .get("background-attachment")
        .map(ComputedValue::css_text)
        .is_some_and(|value| {
            crate::css::split_top_level_commas(&value)
                .into_iter()
                .any(|value| value.trim().eq_ignore_ascii_case("fixed"))
        })
}

pub(super) struct RetainedPaintCollector {
    plan: ExtractionPlan,
    runs: Vec<RetainedPaintRun>,
    document_runs: usize,
    retained_bytes: usize,
    error: Option<RetainedPaintError>,
    layer_mode: RetainedLayerMode,
    active_document_layer_guard:
        Option<(RetainedPaintRunIdentity, Arc<std::sync::atomic::AtomicBool>)>,
    active_fixed_layer_guard:
        Option<(RetainedPaintRunIdentity, Arc<std::sync::atomic::AtomicBool>)>,
    #[cfg(test)]
    fail_after_runs: Option<usize>,
}

enum RetainedLayerMode {
    None,
    Plan { runs: Vec<RetainedRunLayerPlan> },
    Capture { plan: Arc<RetainedLayerPlan> },
}

fn retained_document_layer_bytes(layers: &[RetainedDocumentLayer]) -> usize {
    layers.iter().fold(0usize, |sum, layer| {
        let Some(image) = &layer.image else {
            return sum;
        };
        sum.saturating_add(image.image.pixels.capacity())
            .saturating_add(
                image
                    .image
                    .animation_regions
                    .capacity()
                    .saturating_mul(std::mem::size_of::<animation::AnimationRegion>()),
            )
    })
}

impl RetainedPaintCollector {
    fn new(mut plan: ExtractionPlan) -> Self {
        let layer_mode = plan
            .document_layers
            .take()
            .map_or(RetainedLayerMode::None, |plan| RetainedLayerMode::Capture {
                plan,
            });
        Self {
            runs: Vec::with_capacity(plan.expected_runs),
            document_runs: 0,
            retained_bytes: 0,
            error: None,
            layer_mode,
            active_document_layer_guard: None,
            active_fixed_layer_guard: None,
            #[cfg(test)]
            fail_after_runs: None,
            plan,
        }
    }

    fn for_layer_plan(mut plan: ExtractionPlan) -> Self {
        plan.document_layers = None;
        let mut collector = Self::new(plan);
        collector.layer_mode = RetainedLayerMode::Plan { runs: Vec::new() };
        collector
    }

    #[cfg(test)]
    fn with_failure_after(mut self, run_count: usize) -> Self {
        self.fail_after_runs = Some(run_count);
        self
    }

    fn fixed_root_id(&self, layout: &LayoutBox) -> Option<usize> {
        self.plan
            .fixed_roots
            .get(&LayoutAddress::of(layout))
            .copied()
    }

    fn layer_origin(&self, space: RetainedPaintSpace) -> (i32, i32) {
        match space {
            RetainedPaintSpace::Document => self.plan.document_origin,
            RetainedPaintSpace::ViewportFixed => (0, 0),
        }
    }

    fn prepare_layer_canvas(
        &mut self,
        canvas: &mut Canvas,
        space: RetainedPaintSpace,
        identity: RetainedPaintRunIdentity,
    ) -> bool {
        let matching_space = matches!(
            (space, identity),
            (
                RetainedPaintSpace::Document,
                RetainedPaintRunIdentity::Document(_)
            ) | (
                RetainedPaintSpace::ViewportFixed,
                RetainedPaintRunIdentity::ViewportFixed(_)
            )
        );
        let occupied = match identity {
            RetainedPaintRunIdentity::Document(_) => self.active_document_layer_guard.is_some(),
            RetainedPaintRunIdentity::ViewportFixed(_) => self.active_fixed_layer_guard.is_some(),
            RetainedPaintRunIdentity::FilterSource(_) => true,
        };
        if !matching_space || occupied {
            trace_layer_mismatch("prepare_slot", || {
                format!(
                    "space={space:?} identity={identity:?} matching_space={matching_space} occupied={occupied} document_active={:?} fixed_active={:?}",
                    self.active_document_layer_guard
                        .as_ref()
                        .map(|(identity, _)| identity),
                    self.active_fixed_layer_guard
                        .as_ref()
                        .map(|(identity, _)| identity)
                )
            });
            self.error = Some(RetainedPaintError::TraversalMismatch);
            return false;
        }
        let origin = self.layer_origin(space);
        let base_first = identity == RetainedPaintRunIdentity::Document(0);
        let result = match (&self.layer_mode, base_first) {
            (_, true) => Ok(None),
            (RetainedLayerMode::None, false) => Ok(None),
            (RetainedLayerMode::Plan { .. }, false) => {
                canvas.install_layer_planner(origin).map(Some)
            }
            (RetainedLayerMode::Capture { plan }, false) => match plan.run(identity) {
                Some(run) => canvas
                    .install_layer_capture(origin, Arc::clone(&run.primitives))
                    .map(Some),
                None => canvas.install_flat_layer_capture(origin).map(Some),
            },
        };
        match result {
            Ok(guard) => match identity {
                RetainedPaintRunIdentity::Document(_) => {
                    self.active_document_layer_guard = guard.map(|guard| (identity, guard));
                }
                RetainedPaintRunIdentity::ViewportFixed(_) => {
                    self.active_fixed_layer_guard = guard.map(|guard| (identity, guard));
                }
                RetainedPaintRunIdentity::FilterSource(_) => unreachable!(),
            },
            Err(error) => {
                self.error = Some(error);
                return false;
            }
        }
        true
    }

    fn complete_layer_canvas(
        &mut self,
        canvas: &mut Canvas,
        identity: RetainedPaintRunIdentity,
    ) -> Result<Option<Vec<RetainedDocumentLayer>>, RetainedPaintError> {
        let base_first = identity == RetainedPaintRunIdentity::Document(0);
        let prepared = match identity {
            RetainedPaintRunIdentity::Document(_) => self.active_document_layer_guard.take(),
            RetainedPaintRunIdentity::ViewportFixed(_) => self.active_fixed_layer_guard.take(),
            RetainedPaintRunIdentity::FilterSource(_) => {
                return Err(RetainedPaintError::TraversalMismatch);
            }
        };
        if prepared
            .as_ref()
            .is_some_and(|(prepared_identity, _)| *prepared_identity != identity)
        {
            trace_layer_mismatch("complete_identity", || {
                format!(
                    "expected={identity:?} prepared={:?}",
                    prepared.as_ref().map(|(identity, _)| identity)
                )
            });
            return Err(RetainedPaintError::TraversalMismatch);
        }
        let prepared_present = prepared.is_some();
        let guard = prepared.map(|(_, guard)| guard);
        let flat_capture = matches!(
            &self.layer_mode,
            RetainedLayerMode::Capture { plan } if !base_first && plan.run(identity).is_none()
        );
        if flat_capture {
            let recorded = canvas.take_flat_layer_capture()?;
            if guard.is_some() != recorded
                || guard.is_some_and(|guard| guard.load(std::sync::atomic::Ordering::Relaxed))
            {
                trace_layer_mismatch("guard", || {
                    format!(
                        "identity={identity:?} guard={} recorded={recorded} base_first={base_first}",
                        prepared_present
                    )
                });
                return Err(RetainedPaintError::TraversalMismatch);
            }
            return Ok(None);
        }
        let recorded = canvas.take_retained_layers()?;
        if guard.is_some() != recorded.is_some()
            || guard.is_some_and(|guard| guard.load(std::sync::atomic::Ordering::Relaxed))
        {
            trace_layer_mismatch("guard", || {
                format!(
                    "identity={identity:?} guard={} recorded={} base_first={base_first}",
                    prepared_present,
                    recorded.is_some()
                )
            });
            return Err(RetainedPaintError::TraversalMismatch);
        }
        match &mut self.layer_mode {
            RetainedLayerMode::None => {
                if recorded.is_some() {
                    return Err(RetainedPaintError::TraversalMismatch);
                }
                Ok(None)
            }
            RetainedLayerMode::Plan { runs } => {
                let Some((primitives, layers)) = recorded else {
                    return Ok(None);
                };
                if layers.len() < 2 {
                    return Ok(Some(Vec::new()));
                }
                if runs.iter().any(|run| run.identity == identity) {
                    return Err(RetainedPaintError::TraversalMismatch);
                }
                runs.push(RetainedRunLayerPlan {
                    identity,
                    base_first: false,
                    primitives: primitives.into(),
                    layers: layers
                        .into_iter()
                        .map(|layer| PlannedLayer {
                            identity: layer.identity,
                            sample_rect: layer.sample_rect,
                            animation_count: layer.animation_count,
                        })
                        .collect::<Vec<_>>()
                        .into(),
                });
                Ok(Some(Vec::new()))
            }
            RetainedLayerMode::Capture { plan } => {
                let expected = plan.run(identity);
                let Some((primitives, layers)) = recorded else {
                    return if base_first && expected.is_none() {
                        Ok(None)
                    } else {
                        Err(RetainedPaintError::TraversalMismatch)
                    };
                };
                let Some(expected) = expected else {
                    return Err(RetainedPaintError::TraversalMismatch);
                };
                if expected.base_first != base_first
                    || primitives.as_slice() != expected.primitives.as_ref()
                    || layers.len() != expected.layers.len()
                    || layers
                        .iter()
                        .zip(expected.layers.iter())
                        .any(|(actual, planned)| {
                            actual.identity != planned.identity
                                || actual.sample_rect != planned.sample_rect
                                || actual.animation_count != planned.animation_count
                        })
                {
                    let primitive_mismatch = primitives
                        .iter()
                        .zip(expected.primitives.iter())
                        .position(|(actual, planned)| actual != planned);
                    let layer_mismatch =
                        layers
                            .iter()
                            .zip(expected.layers.iter())
                            .position(|(actual, planned)| {
                                actual.identity != planned.identity
                                    || actual.sample_rect != planned.sample_rect
                                    || actual.animation_count != planned.animation_count
                            });
                    trace_layer_mismatch("capture_plan", || {
                        format!(
                            "identity={identity:?} base={base_first}/{} primitives={}/{} primitive_mismatch={primitive_mismatch:?} layers={}/{} layer_mismatch={layer_mismatch:?}",
                            expected.base_first,
                            primitives.len(),
                            expected.primitives.len(),
                            layers.len(),
                            expected.layers.len()
                        )
                    });
                    return Err(RetainedPaintError::TraversalMismatch);
                }
                Ok(Some(layers))
            }
        }
    }

    fn prepare_fixed_canvas(&mut self, canvas: &mut Canvas, fixed_root_id: usize) -> bool {
        self.prepare_layer_canvas(
            canvas,
            RetainedPaintSpace::ViewportFixed,
            RetainedPaintRunIdentity::ViewportFixed(fixed_root_id),
        )
    }

    fn flush_document(&mut self, canvas: &mut Canvas) -> bool {
        if self.error.is_some() {
            return false;
        }
        let replacement = Canvas::new(canvas.width(), canvas.height());
        let completed = std::mem::replace(canvas, replacement);
        let identity = RetainedPaintRunIdentity::Document(self.document_runs);
        if !self.push(completed, RetainedPaintSpace::Document, identity) {
            return false;
        }
        self.document_runs += 1;
        if !self.prepare_layer_canvas(
            canvas,
            RetainedPaintSpace::Document,
            RetainedPaintRunIdentity::Document(self.document_runs),
        ) {
            return false;
        }
        true
    }

    fn push_fixed(&mut self, canvas: Canvas, fixed_root_id: usize) -> bool {
        self.push(
            canvas,
            RetainedPaintSpace::ViewportFixed,
            RetainedPaintRunIdentity::ViewportFixed(fixed_root_id),
        )
    }

    fn push(
        &mut self,
        mut canvas: Canvas,
        space: RetainedPaintSpace,
        identity: RetainedPaintRunIdentity,
    ) -> bool {
        #[cfg(test)]
        if self.fail_after_runs == Some(self.runs.len()) {
            self.error = Some(RetainedPaintError::CollectionFailed);
            return false;
        }
        let recorded_layers = match self.complete_layer_canvas(&mut canvas, identity) {
            Ok(layers) => layers,
            Err(error) => {
                self.error = Some(error);
                return false;
            }
        };
        let recorded = recorded_layers.is_some();
        let base_first = identity == RetainedPaintRunIdentity::Document(0) && recorded;
        let document_layers = recorded_layers.unwrap_or_default();
        let layer_bytes = retained_document_layer_bytes(&document_layers);
        let surface_bytes = if recorded && !base_first {
            0
        } else {
            self.plan.surface_bytes
        };
        let animation_bytes = if recorded && !base_first {
            0
        } else {
            canvas
                .animation_regions
                .capacity()
                .checked_mul(std::mem::size_of::<animation::AnimationRegion>())
                .unwrap_or(usize::MAX)
        };
        let next_bytes = self
            .retained_bytes
            .checked_add(surface_bytes)
            .and_then(|bytes| bytes.checked_add(animation_bytes))
            .and_then(|bytes| bytes.checked_add(layer_bytes));
        if canvas.width() != self.plan.width
            || canvas.height() != self.plan.height
            || self.runs.len() >= self.plan.expected_runs
            || self.runs.len() >= MAX_RETAINED_RUNS
            || next_bytes.is_none_or(|bytes| {
                !super::gradient_tiles::retained_peak_fits(bytes, MAX_RETAINED_BYTES)
            })
        {
            self.error = Some(RetainedPaintError::CollectionFailed);
            return false;
        }
        self.retained_bytes = next_bytes.unwrap_or(self.retained_bytes);
        let image = if recorded && !base_first {
            Canvas::new(0, 0).into_image()
        } else {
            canvas.into_image()
        };
        self.runs.push(RetainedPaintRun {
            space,
            identity,
            image,
            document_layers,
            filter_source: None,
        });
        true
    }

    fn finish(mut self, canvas: Canvas) -> Result<RetainedPaintFrame, PaintAttemptFailure> {
        if self.error.is_none() {
            let identity = RetainedPaintRunIdentity::Document(self.document_runs);
            self.push(canvas, RetainedPaintSpace::Document, identity);
            if self.error.is_none()
                && (self.active_document_layer_guard.is_some()
                    || self.active_fixed_layer_guard.is_some())
            {
                self.error = Some(RetainedPaintError::TraversalMismatch);
            }
        }
        if let Some(error) = self.error {
            return Err(PaintAttemptFailure {
                reason: error,
                discarded_runs: self.runs.len(),
            });
        }
        if self.runs.len() != self.plan.expected_runs {
            return Err(PaintAttemptFailure {
                reason: RetainedPaintError::TraversalMismatch,
                discarded_runs: self.runs.len(),
            });
        }
        Ok(RetainedPaintFrame {
            width: self.plan.width,
            height: self.plan.height,
            runs: self.runs,
        })
    }

    fn finish_layer_plan(
        mut self,
        canvas: Canvas,
    ) -> Result<RetainedLayerPlanningResult, PaintAttemptFailure> {
        if self.error.is_none() {
            let identity = RetainedPaintRunIdentity::Document(self.document_runs);
            self.push(canvas, RetainedPaintSpace::Document, identity);
            if self.error.is_none()
                && (self.active_document_layer_guard.is_some()
                    || self.active_fixed_layer_guard.is_some())
            {
                self.error = Some(RetainedPaintError::TraversalMismatch);
            }
        }
        if let Some(error) = self.error {
            return Err(PaintAttemptFailure {
                reason: error,
                discarded_runs: self.runs.len(),
            });
        }
        if self.runs.len() != self.plan.expected_runs {
            return Err(PaintAttemptFailure {
                reason: RetainedPaintError::TraversalMismatch,
                discarded_runs: self.runs.len(),
            });
        }
        let RetainedLayerMode::Plan { runs } = self.layer_mode else {
            return Err(PaintAttemptFailure {
                reason: RetainedPaintError::TraversalMismatch,
                discarded_runs: self.runs.len(),
            });
        };
        // Document(0) is painted without a layer recorder. When no fixed or
        // filter root splits the traversal, its full canvas is already the
        // exact current sample. Other planning runs contain metadata rather
        // than captured layer pixels and must still be painted in capture mode.
        let current_frame = (self.plan.expected_runs == 1).then(|| RetainedPaintFrame {
            width: self.plan.width,
            height: self.plan.height,
            runs: self.runs,
        });
        Ok(RetainedLayerPlanningResult {
            layers: RetainedLayerPlan { runs },
            plan: self.plan,
            current_frame,
        })
    }
}

struct RetainedLayerPlanningResult {
    layers: RetainedLayerPlan,
    plan: ExtractionPlan,
    current_frame: Option<RetainedPaintFrame>,
}

struct PaintAttemptFailure {
    reason: RetainedPaintError,
    discarded_runs: usize,
}

struct OverscanBuilder {
    width: u32,
    height: u32,
    plan: ResolvedVerticalOverscanPlan,
    runs: Vec<RetainedOverscanRun>,
}

fn overscan_sample_coverage(
    space: RetainedPaintSpace,
    document_x: i32,
    sample_y: i32,
    width: u32,
    height: u32,
) -> Result<OverscanRasterRect, RetainedPaintError> {
    let (left, top) = match space {
        RetainedPaintSpace::Document => (i64::from(document_x), i64::from(sample_y)),
        RetainedPaintSpace::ViewportFixed => (0, 0),
    };
    Ok(OverscanRasterRect {
        left,
        top,
        right: left
            .checked_add(i64::from(width))
            .ok_or(RetainedPaintError::Capacity)?,
        bottom: top
            .checked_add(i64::from(height))
            .ok_or(RetainedPaintError::Capacity)?,
    })
}

fn retained_sparse_tile(
    image: RetainedSparseImage,
    origin: (i32, i32),
) -> Result<RetainedOverscanTile, RetainedPaintError> {
    Ok(RetainedOverscanTile {
        origin: (
            origin
                .0
                .checked_add(image.origin.0)
                .ok_or(RetainedPaintError::Capacity)?,
            origin
                .1
                .checked_add(image.origin.1)
                .ok_or(RetainedPaintError::Capacity)?,
        ),
        image: image.image,
    })
}

fn overscan_layer(
    layer: RetainedDocumentLayer,
    origin: (i32, i32),
    coverage: OverscanRasterRect,
) -> Result<RetainedOverscanLayer, RetainedPaintError> {
    let tile = layer
        .image
        .map(|image| retained_sparse_tile(image, origin))
        .transpose()?;
    Ok(RetainedOverscanLayer {
        identity: layer.identity,
        sample_rect: layer.sample_rect,
        samples: vec![RetainedLayerSample { coverage, tile }],
    })
}

fn retained_image_exact(left: &Image, right: &Image) -> bool {
    left == right && left.animation_regions == right.animation_regions
}

fn retained_sparse_exact(
    retained: Option<&RetainedOverscanTile>,
    candidate: Option<&RetainedOverscanTile>,
) -> bool {
    match (retained, candidate) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.origin == right.origin && retained_image_exact(&left.image, &right.image)
        }
        _ => false,
    }
}

impl OverscanBuilder {
    fn from_current(
        frame: RetainedPaintFrame,
        plan: ResolvedVerticalOverscanPlan,
    ) -> Result<Self, RetainedPaintError> {
        let current_y = plan.sample_y[plan.current_index];
        let runs = frame
            .runs
            .into_iter()
            .map(|run| {
                let origin = match run.space {
                    RetainedPaintSpace::Document => (plan.document_x, current_y),
                    RetainedPaintSpace::ViewportFixed => (0, 0),
                };
                if let Some(filter_source) = run.filter_source {
                    return Ok(RetainedOverscanRun {
                        space: run.space,
                        identity: run.identity,
                        tiles: Vec::new(),
                        sample_coverages: Vec::new(),
                        document_layers: Vec::new(),
                        filter_source: Some(filter_source),
                    });
                }
                let coverage = overscan_sample_coverage(
                    run.space,
                    plan.document_x,
                    current_y,
                    frame.width,
                    frame.height,
                )?;
                let layers = run
                    .document_layers
                    .into_iter()
                    .map(|layer| overscan_layer(layer, origin, coverage))
                    .collect::<Result<Vec<_>, _>>()?;
                let tiles = if run.image.width() == 0 && run.image.height() == 0 {
                    Vec::new()
                } else {
                    vec![RetainedOverscanTile {
                        origin,
                        image: run.image,
                    }]
                };
                Ok(RetainedOverscanRun {
                    space: run.space,
                    identity: run.identity,
                    tiles,
                    sample_coverages: vec![coverage],
                    document_layers: layers,
                    filter_source: None,
                })
            })
            .collect::<Result<Vec<_>, RetainedPaintError>>()?;
        if runs.is_empty() {
            return Err(RetainedPaintError::OverscanRunMismatch);
        }
        let builder = Self {
            width: frame.width,
            height: frame.height,
            plan,
            runs,
        };
        builder.validate_bytes()?;
        Ok(builder)
    }

    fn validate_bytes(&self) -> Result<(), RetainedPaintError> {
        let bytes = self.runs.iter().fold(0usize, |sum, run| {
            let pixels = run.tiles.iter().fold(0usize, |sum, tile| {
                sum.saturating_add(tile.image.pixels.capacity())
                    .saturating_add(
                        tile.image
                            .animation_regions
                            .capacity()
                            .saturating_mul(std::mem::size_of::<animation::AnimationRegion>()),
                    )
            });
            let layers = run.document_layers.iter().fold(0usize, |sum, layer| {
                layer.samples.iter().fold(sum, |sum, sample| {
                    sample.tile.as_ref().map_or(sum, |tile| {
                        sum.saturating_add(tile.image.pixels.capacity())
                            .saturating_add(
                                tile.image.animation_regions.capacity().saturating_mul(
                                    std::mem::size_of::<animation::AnimationRegion>(),
                                ),
                            )
                    })
                })
            });
            sum.saturating_add(pixels)
                .saturating_add(layers)
                .saturating_add(
                    run.filter_source
                        .as_ref()
                        .map_or(0, FilterSourceLayer::bytes),
                )
        });
        if !super::gradient_tiles::retained_peak_fits(
            bytes
                .saturating_add(self.plan.scratch_bytes)
                .saturating_add(self.plan.metadata_bytes),
            MAX_RETAINED_BYTES,
        ) {
            return Err(RetainedPaintError::Capacity);
        }
        Ok(())
    }

    fn tile_count(&self) -> usize {
        self.runs
            .iter()
            .map(|run| {
                run.filter_source
                    .as_ref()
                    .map_or(run.sample_coverages.len(), FilterSourceLayer::sample_count)
            })
            .sum()
    }

    fn append(
        &mut self,
        frame: RetainedPaintFrame,
        sample_y: i32,
    ) -> Result<(), RetainedPaintError> {
        if frame.width != self.width
            || frame.height != self.height
            || frame.runs.len() != self.runs.len()
        {
            return Err(RetainedPaintError::OverscanRunMismatch);
        }
        for (retained, candidate) in self.runs.iter_mut().zip(frame.runs) {
            if retained.space != candidate.space || retained.identity != candidate.identity {
                return Err(RetainedPaintError::OverscanRunMismatch);
            }
            if let Some(source) = retained.filter_source.as_mut() {
                source.append(
                    candidate
                        .filter_source
                        .ok_or(RetainedPaintError::OverscanRunMismatch)?,
                )?;
                continue;
            }
            if candidate.filter_source.is_some() {
                return Err(RetainedPaintError::OverscanRunMismatch);
            }
            if retained.document_layers.len() != candidate.document_layers.len() {
                return Err(RetainedPaintError::OverscanRunMismatch);
            }
            let coverage = overscan_sample_coverage(
                retained.space,
                self.plan.document_x,
                sample_y,
                self.width,
                self.height,
            )?;
            let origin = match retained.space {
                RetainedPaintSpace::Document => (self.plan.document_x, sample_y),
                RetainedPaintSpace::ViewportFixed => (0, 0),
            };
            match retained.space {
                RetainedPaintSpace::Document => {
                    retained.sample_coverages.push(coverage);
                    if candidate.image.width() != 0 || candidate.image.height() != 0 {
                        retained.tiles.push(RetainedOverscanTile {
                            origin,
                            image: candidate.image,
                        });
                    }
                    for (layer, candidate) in retained
                        .document_layers
                        .iter_mut()
                        .zip(candidate.document_layers)
                    {
                        if layer.identity != candidate.identity
                            || layer.sample_rect != candidate.sample_rect
                        {
                            return Err(RetainedPaintError::OverscanRunMismatch);
                        }
                        layer.samples.push(RetainedLayerSample {
                            coverage,
                            tile: candidate
                                .image
                                .map(|image| retained_sparse_tile(image, origin))
                                .transpose()?,
                        });
                    }
                }
                RetainedPaintSpace::ViewportFixed => {
                    let candidate_base =
                        if candidate.image.width() == 0 && candidate.image.height() == 0 {
                            None
                        } else {
                            Some(RetainedOverscanTile {
                                origin,
                                image: candidate.image,
                            })
                        };
                    if !retained_sparse_exact(retained.tiles.first(), candidate_base.as_ref()) {
                        return Err(RetainedPaintError::FixedRunChanged);
                    }
                    for (layer, candidate) in retained
                        .document_layers
                        .iter()
                        .zip(candidate.document_layers)
                    {
                        let candidate_tile = candidate
                            .image
                            .map(|image| retained_sparse_tile(image, origin))
                            .transpose()?;
                        if layer.identity != candidate.identity
                            || layer.sample_rect != candidate.sample_rect
                            || !retained_sparse_exact(
                                layer.samples[0].tile.as_ref(),
                                candidate_tile.as_ref(),
                            )
                        {
                            return Err(RetainedPaintError::FixedRunChanged);
                        }
                    }
                }
            }
        }
        self.validate_bytes()
    }

    fn finish(mut self) -> Result<RetainedOverscanFrame, RetainedPaintError> {
        self.validate_bytes()?;
        if self.tile_count() != self.plan.final_tiles {
            return Err(RetainedPaintError::OverscanRunMismatch);
        }
        for run in &mut self.runs {
            if run.space == RetainedPaintSpace::Document {
                run.tiles.sort_by_key(|tile| tile.origin.1);
                run.sample_coverages.sort_by_key(|coverage| coverage.top);
                for layer in &mut run.document_layers {
                    layer.samples.sort_by_key(|sample| sample.coverage.top);
                }
            }
        }
        let first_y = self.plan.sample_y[0];
        let last_y = *self
            .plan
            .sample_y
            .last()
            .ok_or(RetainedPaintError::InvalidOverscanPlan)?;
        let current_y = self.plan.sample_y[self.plan.current_index];
        Ok(RetainedOverscanFrame {
            width: self.width,
            height: self.height,
            raster_scale: 1.0,
            document_x: self.plan.document_x,
            first_y,
            last_y,
            current_y,
            runs: self.runs,
            empty_filter_sources: self.plan.empty_filter_sources,
        })
    }
}

/// Switches a classified viewport-fixed root into its own full-viewport run.
/// Returns true when the caller must not paint the subtree again.
pub(super) fn intercept_viewport_fixed_root(
    canvas: &mut Canvas,
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    context: PaintContext<'_>,
    options: PaintBoxOptions,
) -> bool {
    if context.retained_destination != PaintDestination::RootDocument {
        return false;
    }
    let Some(collector) = context.retained_collector else {
        return false;
    };
    let Some(fixed_root_id) = collector.borrow().fixed_root_id(layout) else {
        return false;
    };
    if !collector.borrow_mut().flush_document(canvas) {
        return true;
    }

    let mut fixed_canvas = Canvas::new(canvas.width(), canvas.height());
    if !collector
        .borrow_mut()
        .prepare_fixed_canvas(&mut fixed_canvas, fixed_root_id)
    {
        return true;
    }
    paint_box_internal(
        &mut fixed_canvas,
        layout,
        resolver,
        PaintContext {
            retained_collector: None,
            retained_destination: PaintDestination::Temporary,
            ..context
        },
        options,
    );
    collector
        .borrow_mut()
        .push_fixed(fixed_canvas, fixed_root_id);
    true
}

/// Runs the real painter with a bounded local collector. Unsupported or failed
/// extraction is discarded and the ordinary painter is rerun for this frame.
pub(crate) fn paint_layout_with_retained_fallback(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    viewport: Rect,
    fonts: &[Arc<Font>],
    web_fonts: Option<&WebFontRegistry>,
) -> RetainedPaintOutcome {
    paint_layout_with_retained_fallback_impl(
        layout,
        resolver,
        viewport,
        fonts,
        web_fonts,
        None,
        &[],
        None,
        None,
        None,
        None,
        None,
        None,
    )
}

/// Extracts a live document with its existing child snapshots, private visited
/// colors, and owner-held shadow cache. Every borrowed input is call-local;
/// only owned pixel runs are returned. Collection does not change CSSOM state.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_layout_with_retained_document_snapshots(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    viewport: Rect,
    fonts: &[Arc<Font>],
    web_fonts: Option<&WebFontRegistry>,
    visited_link_ids: &[usize],
    nested_documents: &HashMap<usize, Image>,
    shadow_masks: &border::FrameShadowMaskCache,
) -> RetainedPaintOutcome {
    paint_layout_with_retained_fallback_impl(
        layout,
        resolver,
        viewport,
        fonts,
        web_fonts,
        Some(nested_documents),
        visited_link_ids,
        Some(shadow_masks),
        None,
        None,
        None,
        None,
        None,
    )
}

/// Captures bounded, vertically contiguous integer scroll samples from one raw
/// layout tree. CSSOM geometry and Window scroll state are not mutated: each
/// sample translates an owned layout clone and runs the existing painter.
///
/// The caller must keep the same image environment, base URL, animation time,
/// child snapshots, and owner cache around this complete call. Returned images
/// own their pixels and playback metadata; no DOM or resolver borrow escapes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_layout_with_vertical_overscan(
    raw_layout: &LayoutBox,
    resolver: &mut StyleResolver,
    viewport: Rect,
    plan: VerticalOverscanPlan,
    fonts: &[Arc<Font>],
    web_fonts: Option<&WebFontRegistry>,
    visited_link_ids: &[usize],
    nested_documents: &HashMap<usize, Image>,
    shadow_masks: &border::FrameShadowMaskCache,
) -> RetainedOverscanOutcome {
    let _gradient_tiles = super::gradient_tiles::freeze_gradient_tiles();
    let dimensions = match SurfaceDimensions::for_viewport(viewport) {
        Ok(dimensions) => dimensions,
        Err(reason) => return RetainedOverscanOutcome::Rejected { reason },
    };
    let view = OverscanPaintView {
        raw_layout,
        viewport,
        fonts,
        web_fonts,
        visited_link_ids,
        nested_documents,
        shadow_masks,
    };
    let validated_plan = match plan.resolve(viewport, dimensions, 1, None, None) {
        Ok(plan) => plan,
        Err(reason) => return RetainedOverscanOutcome::Rejected { reason },
    };
    let preflight = match view.expected_runs(resolver, dimensions, &validated_plan) {
        Ok(preflight) => preflight,
        Err(reason) => return view.fallback(resolver, &validated_plan, reason, 0),
    };
    let plan = match plan.resolve(
        viewport,
        dimensions,
        preflight.expected_runs,
        Some(&preflight.filters),
        Some(preflight.document_layers.as_ref()),
    ) {
        Ok(plan) => plan,
        Err(reason) => return RetainedOverscanOutcome::Rejected { reason },
    };
    capture_vertical_overscan(
        view,
        resolver,
        plan,
        preflight.sticky,
        preflight.filters,
        preflight.document_layers,
        preflight.current_frame,
    )
}

#[allow(clippy::too_many_arguments)]
fn capture_vertical_overscan(
    view: OverscanPaintView<'_>,
    resolver: &mut StyleResolver,
    plan: ResolvedVerticalOverscanPlan,
    sticky: StickyPlan,
    filters: FilterSourcePlan,
    document_layers: Arc<RetainedLayerPlan>,
    current_frame: Option<(ExtractionPlan, RetainedPaintFrame)>,
) -> RetainedOverscanOutcome {
    let current_y = plan.sample_y[plan.current_index];
    let current = match current_frame {
        Some((mut current_plan, frame)) => {
            // Keep the capture-mode allocation guard even though the base
            // document pixels need no second traversal. Do this after the
            // complete overscan plan resolves, matching normal sample order.
            if let Err(reason) = current_plan.install_document_layers(Arc::clone(&document_layers))
            {
                return view.fallback(resolver, &plan, reason, 0);
            }
            RetainedPaintOutcome::Extracted(frame)
        }
        None => view.sample(
            resolver,
            (plan.document_x, current_y),
            &sticky,
            &filters,
            &document_layers,
        ),
    };
    let frame = match current {
        RetainedPaintOutcome::Extracted(frame) => frame,
        RetainedPaintOutcome::Fallback {
            canvas,
            reason,
            discarded_runs,
        } => {
            return RetainedOverscanOutcome::Fallback {
                canvas,
                reason,
                discarded_tiles: discarded_runs,
            };
        }
        RetainedPaintOutcome::Rejected { reason } => {
            return RetainedOverscanOutcome::Rejected { reason };
        }
    };
    let current_runs = frame.runs.len();
    let mut builder = match OverscanBuilder::from_current(frame, plan.clone()) {
        Ok(builder) => builder,
        Err(reason) => return view.fallback(resolver, &plan, reason, current_runs),
    };
    for (index, &sample_y) in plan.sample_y.iter().enumerate() {
        if index == plan.current_index {
            continue;
        }
        let before = builder.tile_count();
        let frame = match view.sample(
            resolver,
            (plan.document_x, sample_y),
            &sticky,
            &filters,
            &document_layers,
        ) {
            RetainedPaintOutcome::Extracted(frame) => frame,
            RetainedPaintOutcome::Fallback {
                reason,
                discarded_runs,
                ..
            } => {
                return view.fallback(
                    resolver,
                    &plan,
                    reason,
                    before.saturating_add(discarded_runs),
                );
            }
            RetainedPaintOutcome::Rejected { reason } => {
                return view.fallback(resolver, &plan, reason, before);
            }
        };
        let candidate_runs = frame.runs.len();
        if let Err(reason) = builder.append(frame, sample_y) {
            return view.fallback(
                resolver,
                &plan,
                reason,
                before.saturating_add(candidate_runs),
            );
        }
    }
    let discarded_tiles = builder.tile_count();
    match builder.finish() {
        Ok(frame) => {
            if std::env::var_os("OMOIKANE_TRACE_RETAINED_FILTER").is_some() {
                let active = frame
                    .runs
                    .iter()
                    .filter(|run| run.filter_source.is_some())
                    .count();
                if active != 0 || !frame.empty_filter_sources.is_empty() {
                    eprintln!(
                        "{}",
                        crate::diagnostics::json_record(
                            "OMOIKANE_RETAINED_SOURCE_FILTER",
                            &serde_json::json!({"result":"extracted","active_groups":active,
                            "inactive_groups":frame.empty_filter_sources.len(),
                            "source_bytes":frame.runs.iter().filter_map(|run|run.filter_source.as_ref())
                                .map(FilterSourceLayer::bytes).sum::<usize>()})
                        )
                    );
                }
            }
            RetainedOverscanOutcome::Extracted(frame)
        }
        Err(reason) => view.fallback(resolver, &plan, reason, discarded_tiles),
    }
}

#[cfg(test)]
pub(super) fn paint_layout_with_injected_collection_failure(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    viewport: Rect,
    fail_after_runs: usize,
) -> RetainedPaintOutcome {
    paint_layout_with_retained_fallback_impl(
        layout,
        resolver,
        viewport,
        &[],
        None,
        None,
        &[],
        None,
        Some(fail_after_runs),
        None,
        None,
        None,
        None,
    )
}

#[cfg(test)]
thread_local! {
    static RETAINED_RASTER_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(super) fn retained_raster_count() -> usize {
    RETAINED_RASTER_COUNT.with(std::cell::Cell::get)
}

#[allow(clippy::too_many_arguments)]
fn paint_retained_canvas(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    viewport: Rect,
    dimensions: SurfaceDimensions,
    fonts: &[Arc<Font>],
    web_fonts: Option<&WebFontRegistry>,
    nested_documents: Option<&HashMap<usize, Image>>,
    external_shadow_masks: Option<&border::FrameShadowMaskCache>,
    collector: &RefCell<RetainedPaintCollector>,
) -> Canvas {
    #[cfg(test)]
    RETAINED_RASTER_COUNT.with(|count| count.set(count.get() + 1));
    let mut canvas = Canvas::new(dimensions.width, dimensions.height);
    let viewport_background = viewport_background_color(layout, resolver);
    if let Some(background) = viewport_background.color {
        canvas.fill_rect(viewport, background);
    }
    if !collector.borrow_mut().prepare_layer_canvas(
        &mut canvas,
        RetainedPaintSpace::Document,
        RetainedPaintRunIdentity::Document(0),
    ) {
        return canvas;
    }
    let local_shadow_masks = external_shadow_masks
        .is_none()
        .then(|| border::FrameShadowMaskCache::new(false));
    let shadow_masks = external_shadow_masks
        .or(local_shadow_masks.as_ref())
        .expect("a local or owner-held shadow-mask cache");
    text::with_render_glyph_cache(|| {
        paint_box_internal(
            &mut canvas,
            layout,
            resolver,
            PaintContext {
                viewport,
                text_fonts: fonts,
                web_fonts,
                nested_documents,
                nested_snapshot_draw_calls: None,
                propagated_background_node: viewport_background.propagated_node,
                shadow_masks: Some(shadow_masks),
                retained_collector: Some(collector),
                retained_destination: PaintDestination::RootDocument,
            },
            PaintBoxOptions {
                inherited_clip: None,
                include_phase_descendants: true,
                paint_decorations: true,
                offset: PaintOffset::default(),
                retained_paint_owner: None,
                retained_atomic_inline: None,
            },
        );
    });
    canvas
}

#[allow(clippy::too_many_arguments)]
fn plan_retained_layers(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    viewport: Rect,
    fonts: &[Arc<Font>],
    web_fonts: Option<&WebFontRegistry>,
    nested_documents: Option<&HashMap<usize, Image>>,
    external_shadow_masks: Option<&border::FrameShadowMaskCache>,
    plan: ExtractionPlan,
) -> Result<RetainedLayerPlanningResult, PaintAttemptFailure> {
    plan.ensure_layer_planning_capacity()
        .map_err(|reason| PaintAttemptFailure {
            reason,
            discarded_runs: 0,
        })?;
    let dimensions = SurfaceDimensions {
        width: plan.width,
        height: plan.height,
        bytes: plan.surface_bytes,
    };
    let collector = RefCell::new(RetainedPaintCollector::for_layer_plan(plan));
    let canvas = paint_retained_canvas(
        layout,
        resolver,
        viewport,
        dimensions,
        fonts,
        web_fonts,
        nested_documents,
        external_shadow_masks,
        &collector,
    );
    collector.into_inner().finish_layer_plan(canvas)
}

#[allow(clippy::too_many_arguments)]
fn paint_layout_with_retained_fallback_impl(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    viewport: Rect,
    fonts: &[Arc<Font>],
    web_fonts: Option<&WebFontRegistry>,
    nested_documents: Option<&HashMap<usize, Image>>,
    visited_link_ids: &[usize],
    external_shadow_masks: Option<&border::FrameShadowMaskCache>,
    fail_after_runs: Option<usize>,
    invariant_clip: Option<InvariantOverflowClip>,
    sticky: Option<&StickyPlan>,
    filter_sources: Option<&FilterSourcePlan>,
    document_layers: Option<Arc<RetainedLayerPlan>>,
) -> RetainedPaintOutcome {
    let _gradient_tiles = super::gradient_tiles::freeze_gradient_tiles();
    #[cfg(not(test))]
    let _ = fail_after_runs;
    let dimensions = match SurfaceDimensions::for_viewport(viewport) {
        Ok(dimensions) => dimensions,
        Err(reason) => return RetainedPaintOutcome::Rejected { reason },
    };
    resolver.begin_visited_paint(visited_link_ids.iter().copied());
    let result = match ExtractionPlan::analyze(
        layout,
        resolver,
        dimensions,
        invariant_clip,
        sticky,
        filter_sources,
    ) {
        Err(reason) => Err(PaintAttemptFailure {
            reason,
            discarded_runs: 0,
        }),
        Ok(mut plan) => (|| {
            let layers = match document_layers {
                Some(layers) => layers,
                None => {
                    let planning = plan_retained_layers(
                        layout,
                        resolver,
                        viewport,
                        fonts,
                        web_fonts,
                        nested_documents,
                        external_shadow_masks,
                        plan,
                    )?;
                    plan = planning.plan;
                    Arc::new(planning.layers)
                }
            };
            plan.install_document_layers(layers)
                .map_err(|reason| PaintAttemptFailure {
                    reason,
                    discarded_runs: 0,
                })?;
            let collector = RetainedPaintCollector::new(plan);
            #[cfg(test)]
            let collector = match fail_after_runs {
                Some(limit) => collector.with_failure_after(limit),
                None => collector,
            };
            let collector = RefCell::new(collector);
            let canvas = paint_retained_canvas(
                layout,
                resolver,
                viewport,
                dimensions,
                fonts,
                web_fonts,
                nested_documents,
                external_shadow_masks,
                &collector,
            );
            collector.into_inner().finish(canvas)
        })(),
    };

    resolver.end_visited_paint();
    match result {
        Ok(frame) => RetainedPaintOutcome::Extracted(frame),
        Err(failure) => RetainedPaintOutcome::Fallback {
            canvas: match nested_documents {
                Some(snapshots) => {
                    paint_layout_with_document_snapshots_internal(
                        layout,
                        resolver,
                        viewport,
                        fonts.to_vec(),
                        web_fonts,
                        visited_link_ids.iter().copied(),
                        snapshots,
                        false,
                        external_shadow_masks,
                    )
                    .0
                }
                None => paint_layout_with_web_fonts(
                    layout,
                    resolver,
                    viewport,
                    fonts.to_vec(),
                    web_fonts,
                ),
            },
            reason: failure.reason,
            discarded_runs: failure.discarded_runs,
        },
    }
}

pub(super) fn intercept_filter_source(
    canvas: &mut Canvas,
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    context: PaintContext<'_>,
    options: PaintBoxOptions,
) -> bool {
    filter_source::intercept(canvas, layout, resolver, context, options)
}

pub(super) fn record_skipped_filter_sources(
    canvas: &mut Canvas,
    layout: &LayoutBox,
    context: PaintContext<'_>,
) {
    filter_source::record_skipped(canvas, layout, context);
}

pub(super) fn capture_filter_source(
    layout: &LayoutBox,
    context: PaintContext<'_>,
    source: &Canvas,
    origin: (i32, i32),
) -> bool {
    filter_source::capture(layout, context, source, origin)
}
