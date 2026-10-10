//! Literal pattern escaping, preserving UTF-16 including unpaired surrogates.
use crate::builtins::string::is_trimmable_whitespace;
use crate::string::CodePoint;
use crate::{Context, JsArgs, JsNativeError, JsResult, JsString, JsValue};

/// Implements RegExp.escape without coercing non-string arguments.
/// <https://tc39.es/ecma262/#sec-regexp.escape>
pub(super) fn escape(_: &JsValue, args: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    let input = args
        .get_or_undefined(0)
        .as_string()
        .ok_or_else(|| JsNativeError::typ().with_message("RegExp.escape requires a string"))?;
    let mut output = Vec::new();
    for point in input.code_points() {
        match point {
            CodePoint::UnpairedSurrogate(unit) => hex_escape(&mut output, unit),
            CodePoint::Unicode(c) => {
                if output.is_empty() && c.is_ascii_alphanumeric() {
                    hex_escape(&mut output, c as u16);
                } else if "^$\\.*+?()[]{}|/".contains(c) {
                    output.push(u16::from(b'\\'));
                    output.push(c as u16);
                } else if let Some(escape) = match c {
                    '\t' => Some(b't'),
                    '\n' => Some(b'n'),
                    '\u{b}' => Some(b'v'),
                    '\u{c}' => Some(b'f'),
                    '\r' => Some(b'r'),
                    _ => None,
                } {
                    output.extend([u16::from(b'\\'), u16::from(escape)]);
                } else if ",-=<>#&!%:;@~'`\"".contains(c) || is_trimmable_whitespace(c) {
                    let mut units = [0; 2];
                    for unit in c.encode_utf16(&mut units) {
                        hex_escape(&mut output, *unit);
                    }
                } else {
                    let mut units = [0; 2];
                    output.extend_from_slice(c.encode_utf16(&mut units));
                }
            }
        }
    }
    Ok(JsString::from(output.as_slice()).into())
}

fn hex_escape(output: &mut Vec<u16>, unit: u16) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    output.push(u16::from(b'\\'));
    let width = if unit <= 0xff {
        output.push(u16::from(b'x'));
        2
    } else {
        output.push(u16::from(b'u'));
        4
    };
    for shift in (0..width).rev() {
        output.push(u16::from(HEX[usize::from((unit >> (shift * 4)) & 15)]));
    }
}
