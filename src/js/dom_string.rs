//! Owned conversion for JavaScript DOMString mutation inputs.

use crate::dom::DomString;
use boa_engine::JsString;

#[cfg(test)]
thread_local! {
    static MATERIALIZED_UTF16_UNITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Decodes a borrowed JavaScript string directly into its scalar owned value.
/// Unpaired surrogates alone require an owned exact UTF-16 buffer.
pub(super) fn from_js(value: &JsString) -> DomString {
    match value.to_std_string() {
        Ok(value) => DomString::Scalar(value),
        Err(_) => {
            let units: Vec<u16> = value.iter().collect();
            #[cfg(test)]
            MATERIALIZED_UTF16_UNITS.with(|count| count.set(count.get() + units.len()));
            DomString::Utf16(units)
        }
    }
}

#[cfg(test)]
pub(super) fn reset_materialized_utf16_units() {
    MATERIALIZED_UTF16_UNITS.with(|count| count.set(0));
}

#[cfg(test)]
pub(super) fn materialized_utf16_units() -> usize {
    MATERIALIZED_UTF16_UNITS.with(std::cell::Cell::get)
}
