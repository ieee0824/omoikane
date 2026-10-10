//! Owned input code points for HTML's UTF-16 string sources.

use crate::dom::DomString;

#[cfg(test)]
thread_local! {
    static UTF16_STAGING_UNITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(super) fn reset_utf16_staging_units() {
    UTF16_STAGING_UNITS.with(|count| count.set(0));
}

#[cfg(test)]
pub(super) fn utf16_staging_units() -> usize {
    UTF16_STAGING_UNITS.with(std::cell::Cell::get)
}

/// A source code point; surrogates remain distinct from U+FFFD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputCodePoint {
    Scalar(char),
    Surrogate(u16),
}

impl InputCodePoint {
    /// Scalar view for lexical decisions; the original unit stays in the cursor.
    pub(crate) fn scalar_view(self) -> char {
        match self {
            Self::Scalar(value) => value,
            Self::Surrogate(_) => '\u{fffd}',
        }
    }

    pub(crate) fn append_utf16(self, output: &mut Vec<u16>) {
        match self {
            Self::Scalar(value) => {
                let mut units = [0; 2];
                output.extend_from_slice(value.encode_utf16(&mut units));
            }
            Self::Surrogate(unit) => output.push(unit),
        }
    }
}

/// Decodes writes without resolving a trailing high surrogate before more input.
#[derive(Debug, Default)]
pub(crate) struct InputDecoder {
    pending_high: Option<u16>,
}

impl InputDecoder {
    /// Scalar input cannot complete a pending high surrogate, even when its
    /// first scalar uses a supplementary-plane UTF-16 encoding.
    pub(crate) fn push_str(&mut self, value: &str, output: &mut Vec<InputCodePoint>) {
        if !value.is_empty() {
            self.finish(output);
            output.extend(value.chars().map(InputCodePoint::Scalar));
        }
    }

    pub(crate) fn push(&mut self, units: &[u16], output: &mut Vec<InputCodePoint>) {
        for &unit in units {
            if let Some(high) = self.pending_high.take() {
                if (0xdc00..=0xdfff).contains(&unit) {
                    let value =
                        0x10000 + ((u32::from(high) - 0xd800) << 10) + (u32::from(unit) - 0xdc00);
                    output.push(InputCodePoint::Scalar(char::from_u32(value).unwrap()));
                    continue;
                }
                output.push(InputCodePoint::Surrogate(high));
            }
            match unit {
                0xd800..=0xdbff => self.pending_high = Some(unit),
                0xdc00..=0xdfff => output.push(InputCodePoint::Surrogate(unit)),
                _ => output.push(InputCodePoint::Scalar(
                    char::from_u32(u32::from(unit)).unwrap(),
                )),
            }
        }
    }

    pub(crate) fn finish(&mut self, output: &mut Vec<InputCodePoint>) {
        if let Some(high) = self.pending_high.take() {
            output.push(InputCodePoint::Surrogate(high));
        }
    }
}

/// Exact owned token text, independent of the scalar state-machine view.
#[derive(Debug)]
pub(crate) struct TextBuffer(DomString);

impl Default for TextBuffer {
    fn default() -> Self {
        Self(DomString::Scalar(String::new()))
    }
}

impl TextBuffer {
    pub(crate) fn push(&mut self, value: char) {
        self.push_code_point(InputCodePoint::Scalar(value));
    }
    pub(crate) fn push_code_point(&mut self, point: InputCodePoint) {
        match (&mut self.0, point) {
            (DomString::Scalar(value), InputCodePoint::Scalar(ch)) => value.push(ch),
            (DomString::Utf16(units), point) => point.append_utf16(units),
            (DomString::Scalar(value), InputCodePoint::Surrogate(unit)) => {
                let mut units: Vec<u16> = value.encode_utf16().collect();
                #[cfg(test)]
                UTF16_STAGING_UNITS.with(|count| count.set(count.get() + units.len()));
                units.push(unit);
                self.0 = DomString::Utf16(units);
            }
        }
    }
    pub(crate) fn push_str(&mut self, value: &str) {
        match &mut self.0 {
            DomString::Scalar(text) => text.push_str(value),
            DomString::Utf16(units) => units.extend(value.encode_utf16()),
        }
    }
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn take(&mut self) -> Option<DomString> {
        if self.0.is_empty() {
            None
        } else {
            Some(match std::mem::take(self).0 {
                DomString::Scalar(value) => DomString::Scalar(value),
                DomString::Utf16(units) => DomString::from_utf16(units),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_write_boundary_preserves_original_units() {
        let units = [
            0xd800, 0x3c, 0xfffd, 0xdc00, 0xd83d, 0xde00, 0xd800, 0xd800, 0x5c, 0x75, 0x44, 0x38,
            0x30, 0x30,
        ];
        for boundary in 0..=units.len() {
            let mut decoder = InputDecoder::default();
            let mut points = Vec::new();
            decoder.push(&units[..boundary], &mut points);
            decoder.push(&[], &mut points);
            decoder.push(&units[boundary..], &mut points);
            decoder.finish(&mut points);
            let mut restored = Vec::new();
            for point in points {
                point.append_utf16(&mut restored);
            }
            assert_eq!(restored, units, "split at {boundary}");
        }
    }

    #[test]
    fn split_pair_waits_for_more_input_but_eof_keeps_unpaired_high() {
        let mut decoder = InputDecoder::default();
        let mut points = Vec::new();
        decoder.push(&[0xd83d], &mut points);
        assert!(points.is_empty());
        decoder.push(&[0xde00], &mut points);
        assert_eq!(points, [InputCodePoint::Scalar('😀')]);
        decoder.push(&[0xd800], &mut points);
        decoder.finish(&mut points);
        decoder.finish(&mut points);
        assert_eq!(
            points,
            [
                InputCodePoint::Scalar('😀'),
                InputCodePoint::Surrogate(0xd800)
            ]
        );
    }

    #[test]
    fn replacement_character_and_surrogate_have_distinct_source_types() {
        let mut decoder = InputDecoder::default();
        let mut points = Vec::new();
        decoder.push(&[0xfffd, 0xd800], &mut points);
        decoder.finish(&mut points);
        assert_eq!(
            points,
            [
                InputCodePoint::Scalar('\u{fffd}'),
                InputCodePoint::Surrogate(0xd800)
            ]
        );
    }

    #[test]
    fn scalar_write_flushes_pending_high_but_empty_write_preserves_it() {
        let mut decoder = InputDecoder::default();
        let mut points = Vec::new();
        decoder.push(&[0xd83d], &mut points);
        decoder.push_str("", &mut points);
        assert!(points.is_empty());
        decoder.push_str("😀é", &mut points);
        assert_eq!(
            points,
            [
                InputCodePoint::Surrogate(0xd83d),
                InputCodePoint::Scalar('😀'),
                InputCodePoint::Scalar('é')
            ]
        );
    }

    #[test]
    fn scalar_token_text_keeps_its_owned_allocation() {
        let mut text = TextBuffer::default();
        text.push_str("scalar é😀 text");
        let DomString::Scalar(value) = &text.0 else {
            panic!("scalar storage expected")
        };
        let pointer = value.as_ptr();
        let Some(DomString::Scalar(value)) = text.take() else {
            panic!("scalar output expected")
        };
        assert_eq!(value.as_ptr(), pointer);
        assert_eq!(value, "scalar é😀 text");
    }

    #[test]
    fn exact_token_storage_normalizes_completed_pair_at_token_boundary() {
        let mut text = TextBuffer::default();
        text.push_code_point(InputCodePoint::Surrogate(0xd83d));
        text.push_code_point(InputCodePoint::Surrogate(0xde00));
        assert_eq!(text.take(), Some(DomString::Scalar("😀".into())));
        text.push('\u{fffd}');
        text.push_code_point(InputCodePoint::Surrogate(0xd800));
        assert_eq!(text.take(), Some(DomString::Utf16(vec![0xfffd, 0xd800])));
    }
}
