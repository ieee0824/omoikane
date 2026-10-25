//! Decimal step alignment over finite binary64 values' shortest decimal strings.
//!
//! Scaling uses at most the finite binary64 exponent range. Big integers keep
//! decimal ties exact without an epsilon or a dependency on the JS runtime.
use num_bigint::BigInt;

fn decimal(value: f64) -> (BigInt, i32) {
    let value = value.to_string();
    let (significand, exponent) = value.split_once(['e', 'E']).unwrap_or((&value, "0"));
    let fraction = significand
        .split_once('.')
        .map_or(0, |(_, digits)| digits.len());
    let integer = significand
        .replace('.', "")
        .parse()
        .expect("finite decimal");
    (
        integer,
        exponent.parse::<i32>().expect("finite exponent") - fraction as i32,
    )
}

/// Rounds to a permitted decimal step, preferring positive infinity on ties.
/// All arguments must be finite and step must be positive.
pub(super) fn snap(value: f64, minimum: f64, maximum: f64, step: f64, base: f64) -> f64 {
    let parts = [value, minimum, maximum, step, base].map(decimal);
    let exponent = parts.iter().map(|(_, exponent)| *exponent).min().unwrap();
    let [value, min, max, step, base] = parts.map(|(coefficient, power)| {
        coefficient * BigInt::from(10u8).pow((power - exponent) as u32)
    });
    let offset = &value - &base;
    let remainder = ((offset % &step) + &step) % &step;
    let lower = &value - &remainder;
    let upper = &lower + &step;
    let allowed = |candidate: &BigInt| candidate >= &min && (max < min || candidate <= &max);
    let selected = match (allowed(&lower), allowed(&upper)) {
        (true, true) => {
            if &remainder * 2 < step {
                lower
            } else {
                upper
            }
        }
        (true, false) => lower,
        (false, true) => upper,
        _ => value.clone(),
    };
    format!("{selected}e{exponent}")
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .unwrap_or_else(|| {
            format!("{value}e{exponent}")
                .parse()
                .expect("original finite value")
        })
}

/// Applies the input stepping algorithm to an exact decimal grid.
pub(super) fn advance(
    value: f64,
    minimum: f64,
    maximum: f64,
    step: f64,
    base: f64,
    count: i32,
    up: bool,
) -> Option<f64> {
    if minimum > maximum {
        return None;
    }
    let parts = [value, minimum, maximum, step, base].map(decimal);
    let exponent = parts.iter().map(|(_, exponent)| *exponent).min()?;
    let [value, min, max, step, base] = parts.map(|(coefficient, power)| {
        coefficient * BigInt::from(10u8).pow((power - exponent) as u32)
    });
    let modulo = |value: BigInt| ((value % &step) + &step) % &step;
    let first = &min + modulo(&base - &min);
    let last = &max - modulo(&max - &base);
    if first > last {
        return None;
    }
    let remainder = modulo(&value - &base);
    let direction_up = if count < 0 { !up } else { up };
    let mut next = if remainder != BigInt::from(0) {
        &value - remainder
            + if direction_up {
                step.clone()
            } else {
                BigInt::from(0)
            }
    } else {
        &value + &step * BigInt::from(count) * if up { 1 } else { -1 }
    };
    next = next.max(first).min(last);
    if (direction_up && next < value) || (!direction_up && next > value) {
        return None;
    }
    format!("{next}e{exponent}")
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

/// Tests an exact decimal step grid before applying the integral type scale.
pub(super) fn on_grid(value: f64, base: f64, step: f64, scale: u64) -> bool {
    let parts = [value, base, step].map(decimal);
    let exponent = parts.iter().map(|(_, exponent)| *exponent).min().unwrap();
    let [value, base, step] = parts.map(|(coefficient, power)| {
        coefficient * BigInt::from(10u8).pow((power - exponent) as u32)
    });
    (value - base) % (step * BigInt::from(scale)) == BigInt::from(0)
}
