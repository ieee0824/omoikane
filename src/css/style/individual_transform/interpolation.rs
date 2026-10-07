//! Interpolation with symbolic percentages and quaternion rotation axes.
use super::*;

pub(super) fn interpolate(
    property: &str,
    mut start: IndividualTransform,
    mut end: IndividualTransform,
    progress: f32,
) -> Option<String> {
    if !progress.is_finite() {
        return None;
    }
    if matches!(
        (&start, &end),
        (IndividualTransform::None, IndividualTransform::None)
    ) {
        return Some("none".into());
    }
    if matches!(start, IndividualTransform::None) {
        start = identity(property, &end)?;
    }
    if matches!(end, IndividualTransform::None) {
        end = identity(property, &start)?;
    }
    let mix = |start: f32, end: f32| start + (end - start) * progress;
    let value = match (start, end) {
        (IndividualTransform::Scale(start), IndividualTransform::Scale(end)) => {
            IndividualTransform::Scale(std::array::from_fn(|i| mix(start[i], end[i])))
        }
        (IndividualTransform::Translate(start), IndividualTransform::Translate(end)) => {
            let values = start
                .iter()
                .zip(&end)
                .map(|(start, end)| {
                    let start = length_math(start)?.scaled(1.0 - progress);
                    let end = length_math(end)?.scaled(progress);
                    Some(computed_length_percentage_math(
                        start.add(end),
                        "left",
                        ResolutionContext::default(),
                    ))
                })
                .collect::<Option<Vec<_>>>()?;
            IndividualTransform::Translate(values.try_into().ok()?)
        }
        (
            IndividualTransform::Rotate {
                axis: start_axis,
                degrees: start,
            },
            IndividualTransform::Rotate {
                axis: end_axis,
                degrees: end,
            },
        ) => rotation(start_axis, start, end_axis, end, progress),
        _ => return None,
    };
    Some(value.serialize())
}

fn identity(property: &str, other: &IndividualTransform) -> Option<IndividualTransform> {
    Some(match property {
        "translate" => IndividualTransform::Translate([
            ComputedValue::Px(0.0),
            ComputedValue::Px(0.0),
            ComputedValue::Px(0.0),
        ]),
        "scale" => IndividualTransform::Scale([1.0; 3]),
        "rotate" => IndividualTransform::Rotate {
            axis: match other {
                IndividualTransform::Rotate { axis, .. } => *axis,
                _ => [0.0, 0.0, 1.0],
            },
            degrees: 0.0,
        },
        _ => return None,
    })
}

fn length_math(value: &ComputedValue) -> Option<LengthPercentageMath> {
    match value {
        ComputedValue::LengthPercentage(value) => Some(value.clone()),
        value => {
            let (px, percentage) = value.linear_length_percentage_components()?;
            Some(LengthPercentageMath::Linear { px, percentage })
        }
    }
}

fn normalized(axis: [f32; 3]) -> [f64; 3] {
    let axis = axis.map(f64::from);
    let length = axis.iter().map(|v| v * v).sum::<f64>().sqrt();
    if length == 0.0 {
        [0.0, 0.0, 1.0]
    } else {
        axis.map(|v| v / length)
    }
}

fn quaternion(axis: [f32; 3], degrees: f32) -> [f64; 4] {
    if axis == [0.0; 3] {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let axis = normalized(axis);
    let (sine, cosine) = (f64::from(degrees).to_radians() / 2.0).sin_cos();
    [axis[0] * sine, axis[1] * sine, axis[2] * sine, cosine]
}

fn rotation(
    start_axis: [f32; 3],
    start: f32,
    end_axis: [f32; 3],
    end: f32,
    progress: f32,
) -> IndividualTransform {
    let dot = normalized(start_axis)
        .iter()
        .zip(normalized(end_axis))
        .map(|(a, b)| a * b)
        .sum::<f64>();
    if start_axis != [0.0; 3] && end_axis != [0.0; 3] && dot.abs() > 1.0 - 1e-12 {
        let end = if dot < 0.0 { -end } else { end };
        return IndividualTransform::Rotate {
            axis: start_axis,
            degrees: start + (end - start) * progress,
        };
    }
    let start = quaternion(start_axis, start);
    let mut end = quaternion(end_axis, end);
    let mut dot = start.iter().zip(end).map(|(a, b)| a * b).sum::<f64>();
    if dot < 0.0 {
        end = end.map(|v| -v);
        dot = -dot;
    }
    let progress = f64::from(progress);
    let weights = if dot > 1.0 - 1e-12 {
        (1.0 - progress, progress)
    } else {
        let angle = dot.clamp(-1.0, 1.0).acos();
        (
            ((1.0 - progress) * angle).sin() / angle.sin(),
            (progress * angle).sin() / angle.sin(),
        )
    };
    let mut value: [f64; 4] = std::array::from_fn(|i| weights.0 * start[i] + weights.1 * end[i]);
    let norm = value.iter().map(|v| v * v).sum::<f64>().sqrt();
    value = value.map(|v| v / norm);
    let sine = value[..3].iter().map(|v| v * v).sum::<f64>().sqrt();
    let axis = if sine < 1e-12 {
        [0.0, 0.0, 1.0]
    } else {
        std::array::from_fn(|i| (value[i] / sine) as f32)
    };
    IndividualTransform::Rotate {
        axis,
        degrees: (2.0 * sine.atan2(value[3])).to_degrees() as f32,
    }
}
