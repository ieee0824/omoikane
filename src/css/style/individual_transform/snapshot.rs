//! Individual transform interpolation for deterministic animation snapshots.
use super::super::*;

#[allow(clippy::too_many_arguments)]
pub(in crate::css::style) fn interpolate(
    steps: &[KeyframeStep],
    progress: f32,
    properties: &mut PropertyMap,
    ctx: ResolutionContext,
    custom: &BTreeMap<String, Value>,
    important: &HashSet<String>,
    underlying: [Option<ComputedValue>; 3],
) {
    let timing = properties
        .get("animation-timing-function")
        .map_or_else(|| "ease".into(), ComputedValue::css_text);
    for (name, base) in ["translate", "rotate", "scale"].into_iter().zip(underlying) {
        if important.contains(name) {
            continue;
        }
        let entries: Vec<_> = steps
            .iter()
            .filter_map(|step| {
                step.declarations
                    .iter()
                    .rev()
                    .find(|d| d.name == name)
                    .map(|d| (step.offset, &d.value))
            })
            .collect();
        if entries.is_empty() {
            continue;
        }
        let lower = entries.iter().rev().find(|(offset, _)| *offset <= progress);
        let upper = entries.iter().find(|(offset, _)| *offset >= progress);
        let resolve = |entry: Option<&(f32, &Value)>| {
            entry
                .map(|(_, value)| {
                    let value = resolve_value_with_custom_properties(value, custom)
                        .unwrap_or_else(|| (*value).clone());
                    compute_value(&value, name, ctx)
                })
                .or_else(|| base.clone())
        };
        let start = lower.map_or(0.0, |e| e.0);
        let end = upper.map_or(1.0, |e| e.0);
        let fraction = if end > start {
            ((progress - start) / (end - start)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let fraction =
            super::super::super::transition::animation_timing_progress(&timing, fraction);
        if let (Some(lower), Some(upper)) = (resolve(lower), resolve(upper)) {
            if let Some(value) = super::super::super::transition::interpolate_property_with_color(
                name, &lower, &upper, fraction, "black",
            ) {
                properties.insert(name, value);
            }
        }
    }
}
