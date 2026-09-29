use super::ReadChar;
use std::io::{self, Read};

/// Bytes read from the source at once. The lexer asks for one byte at a time,
/// so reading through `io::Bytes` made every byte a separate `read` call.
const READ_CHUNK_SIZE: usize = 8 * 1024;

/// Input for UTF-8 encoded sources.
#[derive(Debug)]
pub struct UTF8Input<R> {
    input: R,
    buffer: Box<[u8]>,
    /// Next unread position in `buffer`.
    position: usize,
    /// Number of valid bytes in `buffer`.
    filled: usize,
}

impl<R: Read> UTF8Input<R> {
    /// Creates a new `UTF8Input` from a UTF-8 encoded source.
    pub(crate) fn new(iter: R) -> Self {
        Self {
            input: iter,
            buffer: vec![0; READ_CHUNK_SIZE].into_boxed_slice(),
            position: 0,
            filled: 0,
        }
    }
}

impl<R: Read> UTF8Input<R> {
    /// Retrieves the next byte
    #[inline]
    fn next_byte(&mut self) -> io::Result<Option<u8>> {
        if self.position == self.filled && !self.refill()? {
            return Ok(None);
        }
        let byte = self.buffer[self.position];
        self.position += 1;
        Ok(Some(byte))
    }

    /// Reads the next chunk, returning `false` at the end of the input.
    #[cold]
    fn refill(&mut self) -> io::Result<bool> {
        loop {
            match self.input.read(&mut self.buffer) {
                Ok(0) => return Ok(false),
                Ok(read) => {
                    self.position = 0;
                    self.filled = read;
                    return Ok(true);
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }
}

impl<R: Read> ReadChar for UTF8Input<R> {
    /// Retrieves the next unchecked char in u32 code point.
    fn next_char(&mut self) -> io::Result<Option<u32>> {
        // Decode UTF-8
        let x = match self.next_byte()? {
            Some(b) if b >= 128 => b,         // UTF-8 codepoint
            b => return Ok(b.map(u32::from)), // ASCII or None
        };

        // Multibyte case follows
        // Decode from a byte combination out of: [[[x y] z] w]
        // NOTE: Performance is sensitive to the exact formulation here
        let init = utf8_first_byte(x, 2);
        let y = self.next_byte()?.unwrap_or(0);
        let mut ch = utf8_acc_cont_byte(init, y);
        if x >= 0xE0 {
            // [[x y z] w] case
            // 5th bit in 0xE0 .. 0xEF is always clear, so `init` is still valid
            let z = self.next_byte()?.unwrap_or(0);
            let y_z = utf8_acc_cont_byte(u32::from(y & CONT_MASK), z);
            ch = (init << 12) | y_z;
            if x >= 0xF0 {
                // [x y z w] case
                // use only the lower 3 bits of `init`
                let w = self.next_byte()?.unwrap_or(0);
                ch = ((init & 7) << 18) | utf8_acc_cont_byte(y_z, w);
            }
        }

        Ok(Some(ch))
    }
}

/// Mask of the value bits of a continuation byte.
const CONT_MASK: u8 = 0b0011_1111;

/// Returns the initial codepoint accumulator for the first byte.
/// The first byte is special, only want bottom 5 bits for width 2, 4 bits
/// for width 3, and 3 bits for width 4.
fn utf8_first_byte(byte: u8, width: u32) -> u32 {
    u32::from(byte & (0x7F >> width))
}

/// Returns the value of `ch` updated with continuation byte `byte`.
fn utf8_acc_cont_byte(ch: u32, byte: u8) -> u32 {
    (ch << 6) | u32::from(byte & CONT_MASK)
}

#[cfg(test)]
mod tests {
    use super::{READ_CHUNK_SIZE, ReadChar, UTF8Input};
    use std::io::{self, Read};

    /// Returns at most `step` bytes per call and is interrupted every other call.
    struct TrickleReader<'a> {
        bytes: &'a [u8],
        step: usize,
        interrupt: bool,
    }

    impl Read for TrickleReader<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.interrupt = !self.interrupt;
            if self.interrupt {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let count = self.step.min(buf.len()).min(self.bytes.len());
            buf[..count].copy_from_slice(&self.bytes[..count]);
            self.bytes = &self.bytes[count..];
            Ok(count)
        }
    }

    fn decode(mut input: impl ReadChar) -> Vec<u32> {
        let mut chars = Vec::new();
        while let Some(ch) = input.next_char().unwrap() {
            chars.push(ch);
        }
        chars
    }

    #[test]
    fn decodes_code_points_across_read_boundaries() {
        // Place multi-byte characters so they straddle the internal chunk size.
        let mut text = "a".repeat(READ_CHUNK_SIZE - 1);
        text.push_str("é€😀z");
        let expected: Vec<u32> = text.chars().map(u32::from).collect();

        assert_eq!(decode(UTF8Input::new(text.as_bytes())), expected);
        for step in [1, 2, 3, 5] {
            let reader = TrickleReader {
                bytes: text.as_bytes(),
                step,
                interrupt: false,
            };
            assert_eq!(decode(UTF8Input::new(reader)), expected, "step {step}");
        }
        assert!(decode(UTF8Input::new(&b""[..])).is_empty());
    }
}
