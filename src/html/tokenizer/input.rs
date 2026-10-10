//! Owned input code points for HTML's UTF-16 string sources.

/// A source code point; surrogates remain distinct from U+FFFD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputCodePoint {
    Scalar(char),
    Surrogate(u16),
}

impl InputCodePoint {
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
#[derive(Debug, Default)]
pub(crate) struct TextBuffer(Vec<u16>);

impl TextBuffer {
    pub(crate) fn push(&mut self, value: char) {
        self.push_code_point(InputCodePoint::Scalar(value));
    }
    pub(crate) fn push_code_point(&mut self, point: InputCodePoint) {
        point.append_utf16(&mut self.0);
    }
    pub(crate) fn push_str(&mut self, value: &str) {
        self.0.extend(value.encode_utf16());
    }
    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }

    pub(crate) fn take(&mut self) -> Option<Vec<u16>> {
        if self.0.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.0))
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
}
