//! String-to-number coercion through Omoikane's public JavaScript runtime.

use omoikane::js::JsRuntime;

#[test]
fn noncanonical_numeric_strings_are_nan_in_the_browser_runtime() {
    let mut runtime = JsRuntime::new().expect("runtime initialization");

    for source in [
        "Number.isNaN(Number('+inf'))",
        "Number.isNaN(Number('-INFINITY'))",
        "Number.isNaN(Number('0x+1'))",
        "Number.isNaN(Number('0b+1'))",
        "Number.isNaN(+'+inf')",
        "Number.isNaN('0x+1' * 1)",
    ] {
        assert_eq!(
            runtime.eval(source).expect("valid JavaScript").as_boolean(),
            Some(true),
            "{source}"
        );
    }

    for (source, expected) in [
        ("Number('+Infinity')", f64::INFINITY),
        ("Number('-Infinity')", f64::NEG_INFINITY),
        ("Number('0x10')", 16.0),
        ("Number('0x1FFFFFFFF')", 8_589_934_591.0),
        ("Number('1e400')", f64::INFINITY),
    ] {
        assert_eq!(
            runtime.eval(source).expect("valid JavaScript").as_number(),
            Some(expected),
            "{source}"
        );
    }
}
