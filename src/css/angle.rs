//! Shared angle syntax and conversions; callers retain their input contracts.

use std::f32::consts::PI;

/// The unitless forms an angle consumer historically accepts.
#[derive(Clone, Copy)]
pub(crate) enum UnitlessZero {
    /// Gradient directions require an angle unit.
    None,
    /// Filters accept only the literal `0`.
    Literal,
    /// Transforms accept numeric zero and normalize its sign.
    Numeric,
    /// Paint angles accept numeric zero and preserve its sign.
    NumericPreservingSign,
}

#[derive(Clone, Copy)]
enum Unit {
    Degrees,
    Radians,
    Gradians,
    Turns,
}

/// A finite number with its original unit, before conversion can overflow.
pub(crate) struct CssAngle {
    number: f32,
    unit: Unit,
}

impl CssAngle {
    /// Identifies gradians for consumers whose legacy suffix precedence rejects them.
    pub(crate) fn is_gradians(&self) -> bool {
        matches!(self.unit, Unit::Gradians)
    }

    /// Parses an angle with explicit unitless-zero and unit-spacing rules.
    pub(crate) fn parse(input: &str, zero: UnitlessZero, trim_number: bool) -> Option<Self> {
        let input = input.trim().to_ascii_lowercase();
        let (number, unit) = if let Some(number) = input.strip_suffix("deg") {
            (number, Unit::Degrees)
        } else if let Some(number) = input.strip_suffix("grad") {
            (number, Unit::Gradians)
        } else if let Some(number) = input.strip_suffix("rad") {
            (number, Unit::Radians)
        } else if let Some(number) = input.strip_suffix("turn") {
            (number, Unit::Turns)
        } else {
            let accepted = match zero {
                UnitlessZero::None => false,
                UnitlessZero::Literal => input == "0",
                UnitlessZero::Numeric | UnitlessZero::NumericPreservingSign => {
                    input.parse::<f32>().ok()? == 0.0
                }
            };
            let number = if matches!(zero, UnitlessZero::NumericPreservingSign) {
                input.parse().ok()?
            } else {
                0.0
            };
            return accepted.then_some(Self {
                number,
                unit: Unit::Degrees,
            });
        };
        let number = if trim_number { number.trim() } else { number };
        let number = number.parse::<f32>().ok()?;
        number.is_finite().then_some(Self { number, unit })
    }

    /// Converts to degrees; callers decide whether conversion overflow is valid.
    pub(crate) fn degrees(&self) -> f32 {
        self.degrees_with_radian_factor(1.0_f32.to_degrees())
    }

    /// Converts with an explicit radian factor to retain legacy rounding.
    pub(crate) fn degrees_with_radian_factor(&self, radian_factor: f32) -> f32 {
        match self.unit {
            Unit::Degrees => self.number,
            Unit::Radians => self.number * radian_factor,
            Unit::Gradians => self.number * 0.9,
            Unit::Turns => self.number * 360.0,
        }
    }

    /// Converts directly to radians, preserving transform's operation order.
    pub(crate) fn radians(&self) -> f32 {
        match self.unit {
            Unit::Degrees => self.number * PI / 180.0,
            Unit::Radians => self.number,
            Unit::Gradians => self.number * PI / 200.0,
            Unit::Turns => self.number * 2.0 * PI,
        }
    }
}
