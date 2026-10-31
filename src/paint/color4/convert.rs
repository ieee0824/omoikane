//! Floating point conversions between CSS color spaces and sRGB.
use super::Space;

type Triple = [f64; 3];
type Matrix = [Triple; 3];

fn mul(matrix: Matrix, value: Triple) -> Triple {
    matrix.map(|row| row.into_iter().zip(value).map(|(a, b)| a * b).sum())
}

const SRGB_XYZ: Matrix = [
    [0.4123907992659595, 0.357584339383878, 0.1804807884018343],
    [0.2126390058715104, 0.715168678767756, 0.0721923153607337],
    [0.0193308187155918, 0.119194779794626, 0.9505321522496607],
];
const XYZ_SRGB: Matrix = [
    [3.2409699419045226, -1.537383177570094, -0.4986107602930034],
    [-0.9692436362808796, 1.8759675015077202, 0.0415550574071756],
    [0.0556300796969937, -0.2039769588889765, 1.0569715142428786],
];
const D50_D65: Matrix = [
    [0.955473421488075, -0.02309845494876471, 0.06325924320057072],
    [
        -0.0283697093338637,
        1.0099953980813041,
        0.021041441191917323,
    ],
    [
        0.012314014864481998,
        -0.020507649298898964,
        1.330365926242124,
    ],
];

fn decode_srgb(v: f64) -> f64 {
    if v.abs() <= 0.04045 {
        v / 12.92
    } else {
        v.signum() * ((v.abs() + 0.055) / 1.055).powf(2.4)
    }
}
fn encode_srgb(v: f64) -> f64 {
    if v.abs() <= 0.0031308 {
        12.92 * v
    } else {
        v.signum() * (1.055 * v.abs().powf(1.0 / 2.4) - 0.055)
    }
}
fn signed_pow(v: f64, exponent: f64) -> f64 {
    v.signum() * v.abs().powf(exponent)
}

pub(super) fn to_srgb(space: Space, values: Triple) -> Triple {
    // Preserve unbounded computed channels for CSSOM, then limit them to the
    // engine's representable range when converting to a raster color.
    let values = values.map(|value| value.clamp(-(f32::MAX as f64), f32::MAX as f64));
    if space == Space::Srgb {
        return gamut_map(values);
    }
    if space == Space::Hsl {
        return gamut_map(hsl(values));
    }
    if space == Space::Hwb {
        return hwb(values);
    }
    let xyz = to_xyz(space, values);
    let rgb = mul(XYZ_SRGB, xyz).map(encode_srgb);
    gamut_map(rgb)
}

/// Converts without gamut mapping so intermediate Color 5 calculations keep
/// out-of-range channels until rasterization.
pub(super) fn between(source: Space, values: Triple, destination: Space) -> Triple {
    if source == destination {
        return values;
    }
    let rgb = if source == Space::Srgb {
        values
    } else if source == Space::Hsl {
        hsl(values)
    } else if source == Space::Hwb {
        hwb(values)
    } else {
        mul(XYZ_SRGB, to_xyz(source, values)).map(encode_srgb)
    };
    from_srgb(destination, rgb)
}

pub(super) fn from_srgb(space: Space, rgb: Triple) -> Triple {
    if space == Space::Srgb {
        return rgb;
    }
    if space == Space::Hsl {
        let max = rgb.into_iter().fold(f64::NEG_INFINITY, f64::max);
        let min = rgb.into_iter().fold(f64::INFINITY, f64::min);
        let lightness = (max + min) / 2.0;
        let delta = max - min;
        if delta == 0.0 {
            return [0.0, 0.0, lightness];
        }
        let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
        let hue = if max == rgb[0] {
            60.0 * ((rgb[1] - rgb[2]) / delta).rem_euclid(6.0)
        } else if max == rgb[1] {
            60.0 * ((rgb[2] - rgb[0]) / delta + 2.0)
        } else {
            60.0 * ((rgb[0] - rgb[1]) / delta + 4.0)
        };
        return [hue, saturation, lightness];
    }
    if space == Space::Hwb {
        let max = rgb.into_iter().fold(f64::NEG_INFINITY, f64::max);
        let min = rgb.into_iter().fold(f64::INFINITY, f64::min);
        let delta = max - min;
        let hue = if delta == 0.0 {
            0.0
        } else if max == rgb[0] {
            60.0 * ((rgb[1] - rgb[2]) / delta).rem_euclid(6.0)
        } else if max == rgb[1] {
            60.0 * ((rgb[2] - rgb[0]) / delta + 2.0)
        } else {
            60.0 * ((rgb[0] - rgb[1]) / delta + 4.0)
        };
        return [hue, min, 1.0 - max];
    }
    if matches!(space, Space::Oklab | Space::Oklch) {
        let lab = rgb_oklab(rgb);
        return if space == Space::Oklch {
            [
                lab[0],
                lab[1].hypot(lab[2]),
                lab[2].atan2(lab[1]).to_degrees().rem_euclid(360.0),
            ]
        } else {
            lab
        };
    }
    let xyz_d65 = mul(SRGB_XYZ, rgb.map(decode_srgb));
    match space {
        Space::XyzD65 => xyz_d65,
        Space::XyzD50 => mul(D65_D50, xyz_d65),
        Space::Lab | Space::Lch => {
            let lab = xyz_lab(mul(D65_D50, xyz_d65));
            if space == Space::Lch {
                [
                    lab[0],
                    lab[1].hypot(lab[2]),
                    lab[2].atan2(lab[1]).to_degrees().rem_euclid(360.0),
                ]
            } else {
                lab
            }
        }
        Space::SrgbLinear => rgb.map(decode_srgb),
        Space::DisplayP3 | Space::DisplayP3Linear => {
            let linear = mul(XYZ_DISPLAY_P3, xyz_d65);
            if space == Space::DisplayP3 {
                linear.map(encode_srgb)
            } else {
                linear
            }
        }
        Space::A98Rgb => mul(XYZ_A98, xyz_d65).map(|value| signed_pow(value, 256.0 / 563.0)),
        Space::ProphotoRgb => mul(XYZ_PROPHOTO, mul(D65_D50, xyz_d65)).map(|value| {
            if value.abs() <= 1.0 / 512.0 {
                value * 16.0
            } else {
                signed_pow(value, 1.0 / 1.8)
            }
        }),
        Space::Rec2020 => mul(XYZ_REC2020, xyz_d65).map(|value| signed_pow(value, 1.0 / 2.4)),
        Space::Hsl | Space::Hwb | Space::Oklab | Space::Oklch | Space::Srgb => unreachable!(),
    }
}

const D65_D50: Matrix = [
    [
        1.0479298208405488,
        0.022946793341019088,
        -0.05019222954313557,
    ],
    [
        0.029627815688159344,
        0.990434484573249,
        -0.01707382502938514,
    ],
    [
        -0.009243058152591178,
        0.015055144896577895,
        0.7518742814281371,
    ],
];
const XYZ_DISPLAY_P3: Matrix = [
    [2.493496911941425, -0.9313836179191239, -0.40271078445071684],
    [
        -0.8294889695615747,
        1.7626640603183463,
        0.023624685841943577,
    ],
    [
        0.03584583024378447,
        -0.07617238926804182,
        0.9568845240076872,
    ],
];
const XYZ_A98: Matrix = [
    [
        2.0415879038107465,
        -0.5650069742788597,
        -0.34473135077832956,
    ],
    [-0.9692436362808798, 1.8759675015077202, 0.04155505740717557],
    [
        0.013444280632031142,
        -0.11836239223101838,
        1.0151749943912054,
    ],
];
const XYZ_PROPHOTO: Matrix = [
    [
        1.3457868816471583,
        -0.25557208737979464,
        -0.05110186497554526,
    ],
    [-0.5446307051249019, 1.5082477428451468, 0.02052744743642139],
    [0.0, 0.0, 1.2119675456389454],
];
const XYZ_REC2020: Matrix = [
    [1.716651187971268, -0.355670783776392, -0.25336628137366],
    [-0.666684351832489, 1.616481236634939, 0.015768545813911],
    [0.017639857445311, -0.042770613257809, 0.942103121235474],
];

fn xyz_lab(xyz: Triple) -> Triple {
    let white = [0.3457 / 0.3585, 1.0, (1.0 - 0.3457 - 0.3585) / 0.3585];
    let f: Triple = std::array::from_fn(|index| {
        let value = xyz[index] / white[index];
        if value > 216.0 / 24389.0 {
            value.cbrt()
        } else {
            (24389.0 / 27.0 * value + 16.0) / 116.0
        }
    });
    [
        116.0 * f[1] - 16.0,
        500.0 * (f[0] - f[1]),
        200.0 * (f[1] - f[2]),
    ]
}

fn to_xyz(space: Space, mut v: Triple) -> Triple {
    if matches!(space, Space::Lch | Space::Oklch) {
        let angle = v[2].to_radians();
        v = [v[0], v[1] * angle.cos(), v[1] * angle.sin()];
    }
    match space {
        Space::Hsl => mul(SRGB_XYZ, hsl(v).map(decode_srgb)),
        Space::Lab | Space::Lch => mul(D50_D65, lab_xyz(v)),
        Space::Oklab | Space::Oklch => oklab_xyz(v),
        Space::XyzD50 => mul(D50_D65, v),
        Space::XyzD65 => v,
        Space::Srgb => mul(SRGB_XYZ, v.map(decode_srgb)),
        Space::SrgbLinear => mul(SRGB_XYZ, v),
        Space::DisplayP3 | Space::DisplayP3Linear => {
            if space == Space::DisplayP3 {
                v = v.map(decode_srgb);
            }
            mul(
                [
                    [0.4865709486482162, 0.2656676931690931, 0.1982172852343625],
                    [0.2289745640697488, 0.6917385218365064, 0.079286914093745],
                    [0.0, 0.0451133818589026, 1.043944368900976],
                ],
                v,
            )
        }
        Space::A98Rgb => mul(
            [
                [0.5766690429101305, 0.1855582379065463, 0.1882286462349947],
                [0.297344975250536, 0.627363566255466, 0.0752914584939979],
                [0.0270313613864123, 0.0706888525358272, 0.9913375368376388],
            ],
            v.map(|c| signed_pow(c, 563.0 / 256.0)),
        ),
        Space::ProphotoRgb => mul(
            D50_D65,
            mul(
                [
                    [0.7977666449006423, 0.13518129740053308, 0.0313477341283922],
                    [0.2880748288194013, 0.711835234241873, 0.00008993693872564],
                    [0.0, 0.0, 0.8251046025104602],
                ],
                v.map(|c| {
                    if c.abs() <= 16.0 / 512.0 {
                        c / 16.0
                    } else {
                        signed_pow(c, 1.8)
                    }
                }),
            ),
        ),
        Space::Rec2020 => mul(
            [
                [0.6369580483012914, 0.1446169035862083, 0.1688809751641721],
                [0.2627002120112671, 0.6779980715188708, 0.059301716469862],
                [0.0, 0.0280726930490874, 1.060985057710791],
            ],
            v.map(|c| signed_pow(c, 2.4)),
        ),
        Space::Hwb => unreachable!(),
    }
}

fn hsl([hue, saturation, lightness]: Triple) -> Triple {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let h = hue.rem_euclid(360.0) / 60.0;
    let x = chroma * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
    let pure = match h.floor() as u32 {
        0 => [chroma, x, 0.0],
        1 => [x, chroma, 0.0],
        2 => [0.0, chroma, x],
        3 => [0.0, x, chroma],
        4 => [x, 0.0, chroma],
        _ => [chroma, 0.0, x],
    };
    let offset = lightness - chroma / 2.0;
    pure.map(|value| value + offset)
}

fn hwb([h, w, b]: Triple) -> Triple {
    if w + b >= 1.0 {
        return [w / (w + b); 3];
    }
    let h = h / 60.0;
    let x = 1.0 - (h.rem_euclid(2.0) - 1.0).abs();
    let pure = match h as u32 {
        0 => [1.0, x, 0.0],
        1 => [x, 1.0, 0.0],
        2 => [0.0, 1.0, x],
        3 => [0.0, x, 1.0],
        4 => [x, 0.0, 1.0],
        _ => [1.0, 0.0, x],
    };
    pure.map(|c| (c * (100.0 - w * 100.0 - b * 100.0) + w * 100.0) / 100.0)
}

fn lab_xyz([l, a, b]: Triple) -> Triple {
    let y = (l + 16.0) / 116.0;
    let f = [y + a / 500.0, y, y - b / 200.0];
    let white = [0.3457 / 0.3585, 1.0, (1.0 - 0.3457 - 0.3585) / 0.3585];
    std::array::from_fn(|i| {
        let cube = f[i].powi(3);
        let normalized = if cube > 216.0 / 24389.0 {
            cube
        } else {
            (116.0 * f[i] - 16.0) / (24389.0 / 27.0)
        };
        normalized * white[i]
    })
}

fn oklab_xyz(v: Triple) -> Triple {
    let lms = mul(
        [
            [1.0, 0.3963377773761749, 0.2158037573099136],
            [1.0, -0.1055613458156586, -0.0638541728258133],
            [1.0, -0.0894841775298119, -1.2914855480194092],
        ],
        v,
    )
    .map(|c| c.powi(3));
    mul(
        [
            [1.2268798758459243, -0.5578149944602171, 0.2813910456659647],
            [-0.0405757452148008, 1.112286803280317, -0.0717110580655164],
            [-0.0763729366746601, -0.4214933324022432, 1.5869240198367816],
        ],
        lms,
    )
}

fn rgb_oklab(v: Triple) -> Triple {
    let lms = mul(
        [
            [0.819022437996703, 0.3619062600528904, -0.1288737815209879],
            [0.0329836539323885, 0.9292868615863434, 0.0361446663506424],
            [0.0481771893596242, 0.2642395317527308, 0.6335478284694309],
        ],
        mul(SRGB_XYZ, v.map(decode_srgb)),
    )
    .map(f64::cbrt);
    mul(
        [
            [0.210454268309314, 0.7936177747023054, -0.0040720430116193],
            [1.9779985324311684, -2.4285922420485799, 0.450593709617411],
            [0.0259040424655478, 0.7827717124575296, -0.8086757549230774],
        ],
        lms,
    )
}

fn in_gamut(rgb: Triple) -> bool {
    rgb.iter().all(|v| *v >= -1e-7 && *v <= 1.0 + 1e-7)
}
fn clip(rgb: Triple) -> Triple {
    rgb.map(|v| v.clamp(0.0, 1.0))
}

/// CSS binary search chroma reduction with local MINDE (JND 0.02 in Oklab).
fn gamut_map(rgb: Triple) -> Triple {
    if in_gamut(rgb) {
        return clip(rgb);
    }
    let lab = rgb_oklab(rgb);
    if lab[0] >= 1.0 {
        return [1.0; 3];
    }
    if lab[0] <= 0.0 {
        return [0.0; 3];
    }
    let clipped_origin = clip(rgb);
    if delta_ok(lab, rgb_oklab(clipped_origin)) < 0.02 {
        return clipped_origin;
    }
    let chroma = lab[1].hypot(lab[2]);
    let mut min = 0.0;
    let mut max = chroma;
    let mut min_in_gamut = true;
    let mut clipped = clip(rgb);
    // Floating point endpoints can stop converging for extreme legal inputs.
    for _ in 0..256 {
        let current = (min + max) / 2.0;
        if max - min <= 0.0001 || current == min || current == max {
            break;
        }
        let candidate = [lab[0], lab[1] * current / chroma, lab[2] * current / chroma];
        let rgb = mul(XYZ_SRGB, oklab_xyz(candidate)).map(encode_srgb);
        if min_in_gamut && in_gamut(rgb) {
            min = current;
            continue;
        }
        clipped = clip(rgb);
        let clipped_lab = rgb_oklab(clipped);
        let difference = delta_ok(candidate, clipped_lab);
        if difference < 0.02 {
            if 0.02 - difference < 0.0001 {
                return clipped;
            }
            min_in_gamut = false;
            min = current;
        } else {
            max = current;
        }
    }
    clipped
}

fn delta_ok(first: Triple, second: Triple) -> f64 {
    first
        .into_iter()
        .zip(second)
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f64>()
        .sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_interpolation_spaces_round_trip_srgb() {
        let rgb = [0.24, 0.51, 0.73];
        for space in [
            Space::Hwb,
            Space::Lab,
            Space::Lch,
            Space::Oklab,
            Space::Oklch,
            Space::Srgb,
            Space::SrgbLinear,
            Space::DisplayP3,
            Space::DisplayP3Linear,
            Space::A98Rgb,
            Space::ProphotoRgb,
            Space::Rec2020,
            Space::XyzD50,
            Space::XyzD65,
        ] {
            let round_trip = to_srgb(space, from_srgb(space, rgb));
            for (actual, expected) in round_trip.into_iter().zip(rgb) {
                assert!(
                    (actual - expected).abs() <= 1e-6,
                    "{space:?}: expected {expected}, got {actual}"
                );
            }
        }
    }
}
