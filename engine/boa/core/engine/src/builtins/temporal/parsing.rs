//! Validation at the boundary to the native Temporal ISO parser.

use crate::{JsNativeError, JsResult, JsString};

/// Rejects fractions exceeding the ISO grammar's nine-digit precision before
/// the native parser can truncate them. Annotation contents are not time fields.
pub(super) fn iso_source(source: &JsString) -> JsResult<String> {
    let source = source.to_std_string_escaped();
    let mut annotation = false;
    let mut fraction_digits = None;
    for byte in source.bytes() {
        if byte == b'[' {
            annotation = true;
        } else if byte == b']' {
            annotation = false;
        }
        if annotation {
            fraction_digits = None;
            continue;
        }
        match byte {
            b'.' | b',' => fraction_digits = Some(0),
            b'0'..=b'9' => {
                if let Some(digits) = fraction_digits.as_mut() {
                    *digits += 1;
                    if *digits > 9 {
                        return Err(JsNativeError::range()
                            .with_message("Temporal time fractions may contain at most nine digits")
                            .into());
                    }
                }
            }
            _ => fraction_digits = None,
        }
    }
    Ok(source)
}
