//! HTML Standard processing-instruction tokenizer states 13.2.5.72–76.
use super::{Cursor, HtmlParseError, TextBuffer, Token, comment_token, is_html_whitespace};

/// Consumes after `<?`; the incremental tokenizer waits for a complete token.
pub(super) fn consume(cursor: &mut Cursor, errors: &mut Vec<HtmlParseError>) -> Option<Token> {
    let Some(first) = cursor.peek() else {
        errors.push(HtmlParseError::UnexpectedEof);
        return None;
    };
    if !first.is_ascii_alphabetic() && first != '_' {
        errors.push(HtmlParseError::InvalidProcessingInstructionTarget);
        return Some(bogus_comment(cursor, String::new()));
    }
    let mut target = String::new();
    while let Some(ch) = cursor.peek() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
            target.push(ch);
            cursor.consume();
        } else {
            break;
        }
    }
    let Some(delimiter) = cursor.peek() else {
        errors.push(HtmlParseError::UnexpectedEof);
        return None;
    };
    if !is_html_whitespace(delimiter) && !matches!(delimiter, '?' | '>') {
        errors.push(HtmlParseError::InvalidProcessingInstructionTarget);
        return Some(bogus_comment(cursor, target));
    }
    if target.eq_ignore_ascii_case("xml") || target.eq_ignore_ascii_case("xml-stylesheet") {
        errors.push(HtmlParseError::DisallowedProcessingInstructionTarget);
        return Some(bogus_comment(cursor, target));
    }
    while cursor.peek().is_some_and(is_html_whitespace) {
        cursor.consume();
    }
    let mut data = TextBuffer::default();
    while let Some(ch) = cursor.consume() {
        if ch == '>' || (ch == '?' && cursor.peek() == Some('>')) {
            if ch == '?' {
                cursor.consume();
            }
            let units = data.take().unwrap_or_default();
            return Some(match String::from_utf16(&units) {
                Ok(data) => Token::ProcessingInstruction { target, data },
                Err(_) => Token::ProcessingInstructionUtf16 {
                    target,
                    data: units,
                },
            });
        }
        data.push_code_point(cursor.consumed_code_point().unwrap());
    }
    errors.push(HtmlParseError::UnexpectedEof);
    None
}

fn bogus_comment(cursor: &mut Cursor, target: String) -> Token {
    let mut data = TextBuffer::default();
    data.push_str(&format!("?{target}"));
    while let Some(ch) = cursor.consume() {
        if ch == '>' {
            break;
        }
        if ch == '\0' {
            data.push('\u{fffd}');
        } else {
            data.push_code_point(cursor.consumed_code_point().unwrap());
        }
    }
    comment_token(data)
}

#[cfg(test)]
mod tests {
    use super::super::{IncrementalTokenizer, Tokenizer};
    use super::*;

    #[test]
    fn invalid_and_xml_specific_targets_remain_bogus_comments() {
        for (source, data) in [
            ("<?9target data?>", "?9target data?"),
            ("<?target:invalid data?>", "?target:invalid data?"),
            ("<?XML version='1.0'?>", "?XML version='1.0'?"),
            ("<?xml-stylesheet href='x'?>", "?xml-stylesheet href='x'?"),
        ] {
            let (tokens, errors) = Tokenizer::new(source).tokenize_with_errors();
            assert_eq!(tokens, vec![Token::Comment(data.into()), Token::Eof]);
            assert_eq!(errors.len(), 1, "{source}");
        }
    }

    #[test]
    fn data_is_literal_and_incomplete_instructions_are_not_emitted() {
        assert_eq!(
            Tokenizer::new("<?_Target-9 a?b &amp;?>").tokenize(),
            vec![
                Token::ProcessingInstruction {
                    target: "_Target-9".into(),
                    data: "a?b &amp;".into()
                },
                Token::Eof,
            ]
        );
        for source in ["<?", "<?target", "<?target data", "<?target data?"] {
            let (tokens, errors) = Tokenizer::new(source).tokenize_with_errors();
            assert_eq!(tokens, vec![Token::Eof]);
            assert_eq!(errors, vec![HtmlParseError::UnexpectedEof]);
        }
    }

    #[test]
    fn every_write_boundary_preserves_processing_instruction_tokens() {
        let source = "<p><?target a?b &amp;?></p>";
        let expected = Tokenizer::new(source).tokenize();
        for boundary in 0..=source.len() {
            let mut tokenizer = IncrementalTokenizer::new();
            tokenizer.push_input(&source[..boundary]);
            let (mut tokens, errors) = tokenizer.drain(false, false);
            assert!(errors.is_empty(), "boundary={boundary}");
            tokenizer.push_input(&source[boundary..]);
            let (tail, errors) = tokenizer.drain(true, false);
            assert!(errors.is_empty(), "boundary={boundary}");
            tokens.extend(tail);
            assert_eq!(tokens, expected, "boundary={boundary}");
        }
    }
}
