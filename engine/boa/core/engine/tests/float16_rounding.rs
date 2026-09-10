//! Binary16 rounding must agree on hosts with and without FP16 instructions.

#![cfg(feature = "float16")]
#![allow(unused_crate_dependencies)]

use boa_engine::{Context, Source};
use float16::f16;

// Decode finite positive binary16 values independently of the conversion crate.
fn positive_value(bits: u16) -> f64 {
    let exponent = i32::from(bits >> 10);
    let fraction = f64::from(bits & 1023);
    if exponent == 0 {
        fraction * 2.0_f64.powi(-24)
    } else {
        (1024.0 + fraction) * 2.0_f64.powi(exponent - 25)
    }
}

fn check_rounding(convert: fn(f64) -> f16) {
    // ECMAScript Math.f16round requires direct binary64 -> binary16,
    // roundTiesToEven. Test both sides of every finite rounding boundary,
    // including the low binary64 bits lost by float16 0.1.5's fallback.
    for lower in 0..0x7bff_u16 {
        let midpoint = (positive_value(lower) + positive_value(lower + 1)) / 2.0;
        let even = if lower & 1 == 0 { lower } else { lower + 1 };
        for (input, expected) in [
            (midpoint.next_down(), lower),
            (midpoint, even),
            (midpoint.next_up(), lower + 1),
        ] {
            for sign in [0, 0x8000] {
                let input = if sign == 0 { input } else { -input };
                assert_eq!(
                    convert(std::hint::black_box(input)).to_bits(),
                    expected | sign,
                    "input={input:?}, binary64={:#018x}",
                    input.to_bits()
                );
            }
        }
    }
    for (input, expected) in [
        (0.0, 0x0000),
        (-0.0, 0x8000),
        (65520.0_f64.next_down(), 0x7bff),
        (65520.0, 0x7c00),
        (65520.0_f64.next_up(), 0x7c00),
        (-65520.0_f64.next_up(), 0xfc00),
        (-65520.0, 0xfc00),
        (-65520.0_f64.next_down(), 0xfbff),
        (f64::INFINITY, 0x7c00),
        (f64::NEG_INFINITY, 0xfc00),
    ] {
        assert_eq!(convert(std::hint::black_box(input)).to_bits(), expected);
    }
    assert!(convert(f64::NAN).is_nan());
}

#[test]
fn software_rounds_every_binary16_boundary_to_nearest_even() {
    check_rounding(f16::from_f64_const);
}

#[test]
fn native_rounds_every_binary16_boundary_to_nearest_even() {
    check_rounding(f16::from_f64);
}

#[test]
fn javascript_float16_conversions_preserve_values_and_order() {
    let result = Context::default()
        .eval(Source::from_bytes(include_str!(
            "assets/float16_rounding.js"
        )))
        .expect("binary16 JavaScript conversions must preserve the specified result");
    assert_eq!(result.as_boolean(), Some(true));
}
