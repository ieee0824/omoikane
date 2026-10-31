//! `globalCompositeOperation` (Compositing and Blending Level 1).

use super::Rgba;

/// A Porter-Duff operator or a blend mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Operator {
    #[default]
    SourceOver,
    SourceIn,
    SourceOut,
    SourceAtop,
    DestinationOver,
    DestinationIn,
    DestinationOut,
    DestinationAtop,
    Lighter,
    Copy,
    Xor,
    Blend(BlendMode),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlendMode {
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl Operator {
    /// Parses a `globalCompositeOperation` keyword.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        use BlendMode::*;
        Some(match value {
            "source-over" => Self::SourceOver,
            "source-in" => Self::SourceIn,
            "source-out" => Self::SourceOut,
            "source-atop" => Self::SourceAtop,
            "destination-over" => Self::DestinationOver,
            "destination-in" => Self::DestinationIn,
            "destination-out" => Self::DestinationOut,
            "destination-atop" => Self::DestinationAtop,
            "lighter" => Self::Lighter,
            "copy" => Self::Copy,
            "xor" => Self::Xor,
            "multiply" => Self::Blend(Multiply),
            "screen" => Self::Blend(Screen),
            "overlay" => Self::Blend(Overlay),
            "darken" => Self::Blend(Darken),
            "lighten" => Self::Blend(Lighten),
            "color-dodge" => Self::Blend(ColorDodge),
            "color-burn" => Self::Blend(ColorBurn),
            "hard-light" => Self::Blend(HardLight),
            "soft-light" => Self::Blend(SoftLight),
            "difference" => Self::Blend(Difference),
            "exclusion" => Self::Blend(Exclusion),
            "hue" => Self::Blend(Hue),
            "saturation" => Self::Blend(Saturation),
            "color" => Self::Blend(Color),
            "luminosity" => Self::Blend(Luminosity),
            _ => return None,
        })
    }

    /// Whether the operator changes destination pixels the source does not
    /// cover, which then composite against transparent black.
    pub(crate) fn is_unbounded(self) -> bool {
        matches!(
            self,
            Self::SourceIn
                | Self::SourceOut
                | Self::DestinationIn
                | Self::DestinationAtop
                | Self::Copy
        )
    }

    /// Composites premultiplied `source` onto premultiplied `destination`.
    pub(crate) fn apply(self, source: Rgba, destination: Rgba) -> Rgba {
        let (sa, da) = (source.0[3], destination.0[3]);
        let porter_duff = |fa: f64, fb: f64| {
            Rgba(std::array::from_fn(|i| {
                (source.0[i] * fa + destination.0[i] * fb).min(1.0)
            }))
        };
        match self {
            Self::SourceOver => porter_duff(1.0, 1.0 - sa),
            Self::SourceIn => porter_duff(da, 0.0),
            Self::SourceOut => porter_duff(1.0 - da, 0.0),
            Self::SourceAtop => porter_duff(da, 1.0 - sa),
            Self::DestinationOver => porter_duff(1.0 - da, 1.0),
            Self::DestinationIn => porter_duff(0.0, sa),
            Self::DestinationOut => porter_duff(0.0, 1.0 - sa),
            Self::DestinationAtop => porter_duff(1.0 - da, sa),
            Self::Lighter => porter_duff(1.0, 1.0),
            Self::Copy => source,
            Self::Xor => porter_duff(1.0 - da, 1.0 - sa),
            Self::Blend(mode) => blend(mode, source, destination),
        }
    }
}

/// Source-over with blending: `co = cs(1 − ad) + cd(1 − as) + as·ad·B(Cs, Cd)`.
fn blend(mode: BlendMode, source: Rgba, destination: Rgba) -> Rgba {
    let (sa, da) = (source.0[3], destination.0[3]);
    let unpremultiply = |color: Rgba, alpha: f64| -> [f64; 3] {
        if alpha > 0.0 {
            std::array::from_fn(|i| (color.0[i] / alpha).min(1.0))
        } else {
            [0.0; 3]
        }
    };
    let (cs, cd) = (unpremultiply(source, sa), unpremultiply(destination, da));
    let mixed = blended(mode, cs, cd);
    let mut out = [0.0; 4];
    for i in 0..3 {
        out[i] = source.0[i] * (1.0 - da) + destination.0[i] * (1.0 - sa) + sa * da * mixed[i];
    }
    out[3] = sa + da * (1.0 - sa);
    Rgba(out)
}

fn blended(mode: BlendMode, cs: [f64; 3], cd: [f64; 3]) -> [f64; 3] {
    use BlendMode::*;
    let separable = |f: fn(f64, f64) -> f64| std::array::from_fn(|i| f(cs[i], cd[i]));
    match mode {
        Multiply => separable(|s, d| s * d),
        Screen => separable(screen),
        Overlay => separable(|s, d| hard_light(d, s)),
        Darken => separable(f64::min),
        Lighten => separable(f64::max),
        ColorDodge => separable(|s, d| {
            if d == 0.0 {
                0.0
            } else if s >= 1.0 {
                1.0
            } else {
                (d / (1.0 - s)).min(1.0)
            }
        }),
        ColorBurn => separable(|s, d| {
            if d >= 1.0 {
                1.0
            } else if s <= 0.0 {
                0.0
            } else {
                1.0 - ((1.0 - d) / s).min(1.0)
            }
        }),
        HardLight => separable(hard_light),
        SoftLight => separable(|s, d| {
            if s <= 0.5 {
                d - (1.0 - 2.0 * s) * d * (1.0 - d)
            } else {
                let dd = if d <= 0.25 {
                    ((16.0 * d - 12.0) * d + 4.0) * d
                } else {
                    d.sqrt()
                };
                d + (2.0 * s - 1.0) * (dd - d)
            }
        }),
        Difference => separable(|s, d| (s - d).abs()),
        Exclusion => separable(|s, d| s + d - 2.0 * s * d),
        Hue => set_lum(set_sat(cs, sat(cd)), lum(cd)),
        Saturation => set_lum(set_sat(cd, sat(cs)), lum(cd)),
        Color => set_lum(cs, lum(cd)),
        Luminosity => set_lum(cd, lum(cs)),
    }
}

fn screen(s: f64, d: f64) -> f64 {
    s + d - s * d
}

fn hard_light(s: f64, d: f64) -> f64 {
    if s <= 0.5 {
        d * 2.0 * s
    } else {
        screen(2.0 * s - 1.0, d)
    }
}

fn lum(c: [f64; 3]) -> f64 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

fn clip_color(c: [f64; 3]) -> [f64; 3] {
    let l = lum(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    let mut out = c;
    if n < 0.0 {
        out = out.map(|v| l + (v - l) * l / (l - n));
    }
    if x > 1.0 {
        out = out.map(|v| l + (v - l) * (1.0 - l) / (x - l));
    }
    out
}

fn set_lum(c: [f64; 3], l: f64) -> [f64; 3] {
    let d = l - lum(c);
    clip_color(c.map(|v| v + d))
}

fn sat(c: [f64; 3]) -> f64 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

fn set_sat(c: [f64; 3], s: f64) -> [f64; 3] {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    if max <= min {
        return [0.0; 3];
    }
    c.map(|v| (v - min) * s / (max - min))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Rgba = Rgba([1.0, 0.0, 0.0, 1.0]);
    const HALF_BLUE: Rgba = Rgba([0.0, 0.0, 0.5, 0.5]);

    #[test]
    fn porter_duff_operators() {
        assert_eq!(
            Operator::SourceOver.apply(HALF_BLUE, RED).0,
            [0.5, 0.0, 0.5, 1.0]
        );
        assert_eq!(Operator::DestinationOver.apply(HALF_BLUE, RED).0, RED.0);
        assert_eq!(
            Operator::SourceIn.apply(HALF_BLUE, Rgba::TRANSPARENT).0,
            [0.0; 4]
        );
        assert_eq!(
            Operator::DestinationOut.apply(HALF_BLUE, RED).0,
            [0.5, 0.0, 0.0, 0.5]
        );
        assert_eq!(Operator::Xor.apply(RED, RED).0, [0.0; 4]);
        assert!(Operator::Copy.is_unbounded() && !Operator::Xor.is_unbounded());
        assert_eq!(Operator::parse("plus-lighter"), None);
    }

    #[test]
    fn blend_modes_mix_opaque_colors() {
        let gray = Rgba([0.5, 0.5, 0.5, 1.0]);
        let multiply = Operator::parse("multiply").unwrap();
        assert_eq!(multiply.apply(gray, RED).0, [0.5, 0.0, 0.0, 1.0]);
        let screen = Operator::parse("screen").unwrap();
        assert_eq!(screen.apply(gray, RED).0, [1.0, 0.5, 0.5, 1.0]);
        let luminosity = Operator::parse("luminosity").unwrap();
        let out = luminosity.apply(gray, RED).0;
        assert!((lum([out[0], out[1], out[2]]) - 0.5).abs() < 1e-9);
    }
}
