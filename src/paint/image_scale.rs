#[cfg(test)]
use std::cell::{Cell, RefCell};
use std::mem::size_of;
use std::ops::Range;

use super::{Color, Image};

pub(super) const MAX_IMAGE_SCALE_PLAN_PAYLOAD_BYTES: usize = 512 * 1024;

#[cfg(test)]
thread_local! {
    static IMAGE_SCALE_PLAN_ATTEMPTS: Cell<usize> = const { Cell::new(0) };
    static IMAGE_SCALE_PLAN_SUCCESSES: Cell<usize> = const { Cell::new(0) };
    static IMAGE_SCALE_SAMPLE_TRACE: RefCell<Option<Vec<(u32, u32, u64)>>> =
        const { RefCell::new(None) };
}

#[cfg(test)]
pub(super) fn take_image_scale_plan_counts() -> (usize, usize) {
    let attempts = IMAGE_SCALE_PLAN_ATTEMPTS.with(|count| count.replace(0));
    let successes = IMAGE_SCALE_PLAN_SUCCESSES.with(|count| count.replace(0));
    (attempts, successes)
}

#[cfg(test)]
pub(super) fn begin_image_scale_sample_trace() {
    IMAGE_SCALE_SAMPLE_TRACE.with(|trace| *trace.borrow_mut() = Some(Vec::new()));
}

#[cfg(test)]
pub(super) fn take_image_scale_sample_trace() -> Vec<(u32, u32, u64)> {
    IMAGE_SCALE_SAMPLE_TRACE.with(|trace| trace.borrow_mut().take().unwrap_or_default())
}

#[cfg(test)]
fn record_image_scale_source_visit(source_y: u32, source_x: u32, weight: f64) {
    IMAGE_SCALE_SAMPLE_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        if let Some(entries) = trace.as_mut() {
            entries.push((source_y, source_x, weight.to_bits()));
        }
    });
}

#[derive(Debug, Clone, Copy)]
struct AxisInput {
    destination_start: i32,
    destination_end: i32,
    destination_origin: f32,
    destination_extent: f32,
    source_extent: u32,
    source_per_destination: f32,
    shrink: bool,
}

impl AxisInput {
    fn geometry(self, destination: i32) -> Option<AxisGeometry> {
        let normalized = (destination as f32 - self.destination_origin) / self.destination_extent;
        let nearest = (normalized * self.source_extent as f32).floor() as i32;
        if nearest < 0 || nearest >= self.source_extent as i32 {
            return None;
        }

        let source_start = if self.shrink {
            ((destination as f32 - self.destination_origin) * self.source_per_destination)
                .clamp(0.0, self.source_extent as f32)
        } else {
            nearest as f32
        };
        let source_end = if self.shrink {
            (((destination + 1) as f32 - self.destination_origin) * self.source_per_destination)
                .clamp(0.0, self.source_extent as f32)
        } else {
            nearest as f32 + 1.0
        };
        Some(AxisGeometry {
            source_start,
            source_end,
            first_source: source_start.floor().max(0.0) as u32,
            source_end_exclusive: source_end.ceil().min(self.source_extent as f32) as u32,
        })
    }

    fn layout(self) -> Option<AxisPlanLayout> {
        let sample_count =
            usize::try_from(self.destination_end.checked_sub(self.destination_start)?).ok()?;
        let mut weight_count = 0usize;
        for destination in self.destination_start..self.destination_end {
            let Some(geometry) = self.geometry(destination) else {
                continue;
            };
            weight_count = weight_count.checked_add(
                usize::try_from(
                    geometry
                        .source_end_exclusive
                        .checked_sub(geometry.first_source)?,
                )
                .ok()?,
            )?;
        }
        Some(AxisPlanLayout {
            sample_count,
            weight_count,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct AxisGeometry {
    source_start: f32,
    source_end: f32,
    first_source: u32,
    source_end_exclusive: u32,
}

#[derive(Debug, Clone, Copy)]
struct AxisPlanLayout {
    sample_count: usize,
    weight_count: usize,
}

impl AxisPlanLayout {
    fn payload_bytes(self) -> Option<usize> {
        self.sample_count
            .checked_mul(size_of::<AxisSample>())?
            .checked_add(self.weight_count.checked_mul(size_of::<f64>())?)
    }
}

#[derive(Debug, Clone, Copy)]
struct AxisSample {
    first_source: u32,
    weight_offset: usize,
    weight_count: usize,
}

#[derive(Debug)]
struct AxisPlan {
    destination_start: i32,
    samples: Vec<AxisSample>,
    weights: Vec<f64>,
}

impl AxisPlan {
    fn try_build(input: AxisInput, layout: AxisPlanLayout) -> Option<Self> {
        let mut samples = Vec::new();
        samples.try_reserve_exact(layout.sample_count).ok()?;
        let mut weights = Vec::new();
        weights.try_reserve_exact(layout.weight_count).ok()?;

        for destination in input.destination_start..input.destination_end {
            let Some(geometry) = input.geometry(destination) else {
                samples.push(AxisSample {
                    first_source: 0,
                    weight_offset: weights.len(),
                    weight_count: 0,
                });
                continue;
            };
            let weight_offset = weights.len();
            for source in geometry.first_source..geometry.source_end_exclusive {
                weights.push(
                    (geometry.source_end.min(source as f32 + 1.0)
                        - geometry.source_start.max(source as f32))
                    .max(0.0) as f64,
                );
            }
            samples.push(AxisSample {
                first_source: geometry.first_source,
                weight_offset,
                weight_count: weights.len() - weight_offset,
            });
        }

        Some(Self {
            destination_start: input.destination_start,
            samples,
            weights,
        })
    }

    fn payload_capacity_bytes(&self) -> Option<usize> {
        self.samples
            .capacity()
            .checked_mul(size_of::<AxisSample>())?
            .checked_add(self.weights.capacity().checked_mul(size_of::<f64>())?)
    }

    fn weighted_sources(&self, destination: i32) -> Option<(u32, &[f64])> {
        let sample_index =
            usize::try_from(destination.checked_sub(self.destination_start)?).ok()?;
        let sample = self.samples.get(sample_index)?;
        let weight_end = sample.weight_offset.checked_add(sample.weight_count)?;
        Some((
            sample.first_source,
            self.weights.get(sample.weight_offset..weight_end)?,
        ))
    }
}

/// Per-draw separable source weights for the existing two-dimensional box filter.
///
/// This owns numeric weights only. Sampling still visits source Y outside source X
/// so that f64 accumulation order remains unchanged.
pub(super) struct ImageScalePlan {
    x: AxisPlan,
    y: AxisPlan,
}

impl ImageScalePlan {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_new(
        destination_x: Range<i32>,
        destination_y: Range<i32>,
        destination_origin_x: f32,
        destination_origin_y: f32,
        destination_width: f32,
        destination_height: f32,
        source_width: u32,
        source_height: u32,
        source_per_destination_x: f32,
        source_per_destination_y: f32,
        shrink_x: bool,
        shrink_y: bool,
    ) -> Option<Self> {
        #[cfg(test)]
        IMAGE_SCALE_PLAN_ATTEMPTS.with(|attempts| attempts.set(attempts.get() + 1));

        let x_input = AxisInput {
            destination_start: destination_x.start,
            destination_end: destination_x.end,
            destination_origin: destination_origin_x,
            destination_extent: destination_width,
            source_extent: source_width,
            source_per_destination: source_per_destination_x,
            shrink: shrink_x,
        };
        let y_input = AxisInput {
            destination_start: destination_y.start,
            destination_end: destination_y.end,
            destination_origin: destination_origin_y,
            destination_extent: destination_height,
            source_extent: source_height,
            source_per_destination: source_per_destination_y,
            shrink: shrink_y,
        };
        let x_layout = x_input.layout()?;
        let y_layout = y_input.layout()?;
        let payload_bytes = x_layout
            .payload_bytes()?
            .checked_add(y_layout.payload_bytes()?)?;
        if payload_bytes > MAX_IMAGE_SCALE_PLAN_PAYLOAD_BYTES {
            return None;
        }

        let plan = Self {
            x: AxisPlan::try_build(x_input, x_layout)?,
            y: AxisPlan::try_build(y_input, y_layout)?,
        };
        if plan.payload_capacity_bytes()? > MAX_IMAGE_SCALE_PLAN_PAYLOAD_BYTES {
            return None;
        }
        #[cfg(test)]
        IMAGE_SCALE_PLAN_SUCCESSES.with(|successes| successes.set(successes.get() + 1));
        Some(plan)
    }

    fn payload_capacity_bytes(&self) -> Option<usize> {
        self.x
            .payload_capacity_bytes()?
            .checked_add(self.y.payload_capacity_bytes()?)
    }

    #[cfg(test)]
    pub(super) fn payload_capacity_bytes_for_test(&self) -> Option<usize> {
        self.payload_capacity_bytes()
    }

    #[cfg(test)]
    pub(super) fn x_weighted_sources_for_test(&self, destination: i32) -> Option<(u32, &[f64])> {
        self.x.weighted_sources(destination)
    }

    #[cfg(test)]
    pub(super) fn y_weighted_sources_for_test(&self, destination: i32) -> Option<(u32, &[f64])> {
        self.y.weighted_sources(destination)
    }

    pub(super) fn sample(&self, image: &Image, destination_x: i32, destination_y: i32) -> Color {
        let Some((first_source_x, x_weights)) = self.x.weighted_sources(destination_x) else {
            return Color::rgba(0, 0, 0, 0);
        };
        let Some((first_source_y, y_weights)) = self.y.weighted_sources(destination_y) else {
            return Color::rgba(0, 0, 0, 0);
        };

        let mut total_weight = 0.0f64;
        let mut alpha_sum = 0.0f64;
        let mut red_sum = 0.0f64;
        let mut green_sum = 0.0f64;
        let mut blue_sum = 0.0f64;
        for (source_y_offset, &y_weight) in y_weights.iter().enumerate() {
            let source_y = first_source_y + source_y_offset as u32;
            for (source_x_offset, &x_weight) in x_weights.iter().enumerate() {
                let source_x = first_source_x + source_x_offset as u32;
                let weight = x_weight * y_weight;
                #[cfg(test)]
                record_image_scale_source_visit(source_y, source_x, weight);
                if weight == 0.0 {
                    continue;
                }
                let index = ((source_y * image.width + source_x) * 4) as usize;
                let alpha = image.pixels[index + 3] as f64;
                total_weight += weight;
                alpha_sum += alpha * weight;
                red_sum += image.pixels[index] as f64 * alpha * weight;
                green_sum += image.pixels[index + 1] as f64 * alpha * weight;
                blue_sum += image.pixels[index + 2] as f64 * alpha * weight;
            }
        }

        if total_weight == 0.0 || alpha_sum == 0.0 {
            return Color::rgba(0, 0, 0, 0);
        }
        let channel = |sum: f64| (sum / alpha_sum).round().clamp(0.0, 255.0) as u8;
        Color::rgba(
            channel(red_sum),
            channel(green_sum),
            channel(blue_sum),
            (alpha_sum / total_weight).round().clamp(0.0, 255.0) as u8,
        )
    }
}
