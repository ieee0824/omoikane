//! Base64 decoding directly from Boa strings, with bounded writes and partial-error results.

use crate::string::JsStr;

#[cfg(test)]
use std::cell::Cell;
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum LastChunk {
    Loose,
    Strict,
    StopBeforePartial,
}

pub(super) struct Decoded {
    pub(super) read: usize,
    pub(super) written: usize,
    pub(super) error: bool,
}

struct Input<'a> {
    value: JsStr<'a>,
    #[cfg(test)]
    units_read: Cell<usize>,
}

impl<'a> Input<'a> {
    fn new(value: JsStr<'a>) -> Self {
        Self {
            value,
            #[cfg(test)]
            units_read: Cell::new(0),
        }
    }

    fn len(&self) -> usize {
        self.value.len()
    }

    fn unit(&self, index: usize) -> u16 {
        #[cfg(test)]
        self.units_read.set(self.units_read.get() + 1);
        self.value.get_expect(index)
    }
}

/// Implements the Uint8Array base64 algorithms without invoking JavaScript.
pub(super) fn decode(
    input: JsStr<'_>,
    url: bool,
    mode: LastChunk,
    max: usize,
    write: impl FnMut(usize, u8),
) -> Decoded {
    decode_input(&Input::new(input), url, mode, max, write)
}

fn decode_input(
    input: &Input<'_>,
    url: bool,
    mode: LastChunk,
    max: usize,
    mut write: impl FnMut(usize, u8),
) -> Decoded {
    let mut result = Decoded {
        read: 0,
        written: 0,
        error: false,
    };
    if max == 0 {
        return result;
    }
    let mut index = 0;
    let mut chunk = [0; 4];
    let mut chunk_len = 0;
    loop {
        let Some(unit) = next_non_space(input, &mut index) else {
            if chunk_len != 0 {
                if mode == LastChunk::StopBeforePartial {
                    return result;
                }
                if mode == LastChunk::Strict || chunk_len == 1 {
                    result.error = true;
                    return result;
                }
                append_chunk(&chunk[..chunk_len], &mut result, &mut write);
            }
            result.read = input.len();
            return result;
        };
        if unit == u16::from(b'=') {
            finish_padding(
                input,
                index,
                &chunk[..chunk_len],
                mode,
                &mut result,
                &mut write,
            );
            return result;
        }
        let Some(value) = digit(unit, url) else {
            result.error = true;
            return result;
        };
        let remaining = max - result.written;
        if (remaining == 1 && chunk_len == 2) || (remaining == 2 && chunk_len == 3) {
            return result;
        }
        chunk[chunk_len] = value;
        chunk_len += 1;
        if chunk_len == 4 {
            append_chunk(&chunk, &mut result, &mut write);
            chunk_len = 0;
            result.read = index;
            if result.written == max {
                return result;
            }
        }
    }
}

fn next_non_space(input: &Input<'_>, index: &mut usize) -> Option<u16> {
    while *index < input.len() {
        let unit = input.unit(*index);
        *index += 1;
        if !matches!(unit, 9 | 10 | 12 | 13 | 32) {
            return Some(unit);
        }
    }
    None
}

fn finish_padding(
    input: &Input<'_>,
    mut index: usize,
    chunk: &[u8],
    mode: LastChunk,
    result: &mut Decoded,
    write: &mut impl FnMut(usize, u8),
) {
    if chunk.len() < 2 {
        result.error = true;
        return;
    }
    skip_space(input, &mut index);
    if chunk.len() == 2 {
        if index == input.len() {
            result.error = mode != LastChunk::StopBeforePartial;
            return;
        }
        if input.unit(index) == u16::from(b'=') {
            index += 1;
            skip_space(input, &mut index);
        }
    }
    if index < input.len()
        || (mode == LastChunk::Strict
            && ((chunk.len() == 2 && chunk[1] & 15 != 0)
                || (chunk.len() == 3 && chunk[2] & 3 != 0)))
    {
        result.error = true;
        return;
    }
    append_chunk(chunk, result, write);
    result.read = input.len();
}

fn skip_space(input: &Input<'_>, index: &mut usize) {
    while *index < input.len() && matches!(input.unit(*index), 9 | 10 | 12 | 13 | 32) {
        *index += 1;
    }
}

fn digit(unit: u16, url: bool) -> Option<u8> {
    match unit {
        65..=90 => Some((unit - 65) as u8),
        97..=122 => Some((unit - 97 + 26) as u8),
        48..=57 => Some((unit - 48 + 52) as u8),
        43 if !url => Some(62),
        47 if !url => Some(63),
        45 if url => Some(62),
        95 if url => Some(63),
        _ => None,
    }
}

fn append_chunk(chunk: &[u8], result: &mut Decoded, write: &mut impl FnMut(usize, u8)) {
    let bits = chunk
        .iter()
        .fold(0u32, |bits, value| (bits << 6) | u32::from(*value))
        << ((4 - chunk.len()) * 6);
    for index in 0..chunk.len() - 1 {
        write(result.written, (bits >> (16 - index * 8)) as u8);
        result.written += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decoding_modes_and_partial_results() {
        for (text, mode, max, expected, read, error) in [
            ("Zm9v", LastChunk::Loose, 9, b"foo".as_slice(), 4, false),
            ("Zg", LastChunk::Loose, 9, b"f".as_slice(), 2, false),
            ("Zg", LastChunk::Strict, 9, b"".as_slice(), 0, true),
            (
                "Zg=",
                LastChunk::StopBeforePartial,
                9,
                b"".as_slice(),
                0,
                false,
            ),
            ("Zh==", LastChunk::Strict, 9, b"".as_slice(), 0, true),
            ("Zm9v!", LastChunk::Loose, 9, b"foo".as_slice(), 4, true),
            (
                "Zm9vZg",
                LastChunk::StopBeforePartial,
                9,
                b"foo".as_slice(),
                4,
                false,
            ),
            ("Zm9v", LastChunk::Loose, 2, b"".as_slice(), 0, false),
            ("Zg==", LastChunk::Loose, 1, b"f".as_slice(), 4, false),
            ("!", LastChunk::Strict, 0, b"".as_slice(), 0, false),
        ] {
            let mut bytes = Vec::new();
            let input = crate::JsString::from(text);
            let result = decode(input.as_str(), false, mode, max, |_, byte| bytes.push(byte));
            assert_eq!(
                (&bytes[..], result.read, result.error),
                (expected, read, error),
                "{text}"
            );
        }
        let mut bytes = Vec::new();
        let input = crate::js_string!("-_8=");
        let result = decode(input.as_str(), true, LastChunk::Strict, 9, |_, byte| {
            bytes.push(byte);
        });
        assert_eq!(bytes, [251, 255]);
        assert!(!result.error);
    }

    #[test]
    fn zero_capacity_does_not_inspect_the_input() {
        let text = vec![b'A'; 1 << 20];
        let input = Input::new(JsStr::latin1(&text));
        let result = decode_input(&input, false, LastChunk::Loose, 0, |_, _| {
            panic!("zero-capacity decode must not write")
        });

        assert_eq!(input.units_read.get(), 0);
        assert_eq!((result.read, result.written, result.error), (0, 0, false));
    }
}
