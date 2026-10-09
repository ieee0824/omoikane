//! Owned source coordinates retained across incremental tokenizer drains.
use super::input::InputCodePoint;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SourcePosition {
    pub(crate) line: u32,
    pub(crate) column: u32,
    after_cr: bool,
}

impl Default for SourcePosition {
    fn default() -> Self {
        Self {
            line: 1,
            column: 1,
            after_cr: false,
        }
    }
}

impl SourcePosition {
    pub(super) fn advance(&mut self, point: InputCodePoint) {
        match point {
            InputCodePoint::Scalar('\r') => {
                self.line = self.line.saturating_add(1);
                self.column = 1;
                self.after_cr = true;
            }
            InputCodePoint::Scalar('\n') => {
                if !self.after_cr {
                    self.line = self.line.saturating_add(1);
                }
                self.column = 1;
                self.after_cr = false;
            }
            point => {
                let width = match point {
                    InputCodePoint::Scalar(ch) => ch.len_utf16() as u32,
                    InputCodePoint::Surrogate(_) => 1,
                };
                self.column = self.column.saturating_add(width);
                self.after_cr = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{IncrementalTokenizer, Token};
    #[test]
    fn script_coordinates_survive_every_input_boundary_and_ignored_markup() {
        let source = "<!-- <script> -->\r\n😀<script data-x='>'>throw 42;</script>";
        for split in source
            .char_indices()
            .map(|(index, _)| index)
            .chain([source.len()])
        {
            let mut tokenizer = IncrementalTokenizer::new();
            tokenizer.push_input(&source[..split]);
            let (first, _) = tokenizer.drain(false, false);
            let mut positions = tokenizer
                .script_source_positions()
                .iter()
                .map(|(index, pos)| {
                    assert!(
                        matches!(&first[*index], Token::StartTag { name, .. } if name == "script")
                    );
                    (pos.line, pos.column)
                })
                .collect::<Vec<_>>();
            tokenizer.push_input(&source[split..]);
            let (second, _) = tokenizer.drain(true, false);
            positions.extend(tokenizer.script_source_positions().iter().map(|(index, pos)| {
                assert!(matches!(&second[*index], Token::StartTag { name, .. } if name == "script"));
                (pos.line, pos.column)
            }));
            assert_eq!(positions, [(2, 22)], "split {split}");
        }
    }
}
