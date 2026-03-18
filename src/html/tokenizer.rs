//! HTML tokenizer.
//!
//! The implementation is intentionally small, but it follows the overall
//! HTML5 tokenizer shape so later tree-construction work can extend it.

use std::fmt;

/// A parsed HTML attribute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute {
    name: String,
    value: String,
}

impl Attribute {
    /// Creates a new attribute.
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }

    /// Returns the attribute name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the attribute value.
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// A parsed HTML doctype token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctypeToken {
    name: Option<String>,
    force_quirks: bool,
}

impl DoctypeToken {
    /// Returns the doctype name, if one was present.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Returns whether the tokenizer marked the doctype as force-quirks.
    pub fn force_quirks(&self) -> bool {
        self.force_quirks
    }
}

/// An HTML tokenizer output token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    StartTag {
        name: String,
        attributes: Vec<Attribute>,
        self_closing: bool,
    },
    EndTag {
        name: String,
    },
    Comment(String),
    Doctype(DoctypeToken),
    Character(String),
    Eof,
}

/// Tokenization errors collected while parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HtmlParseError {
    UnexpectedEof,
    MissingTagName,
    InvalidCharacterReference,
    InvalidDoctype,
}

impl fmt::Display for HtmlParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof => write!(f, "unexpected EOF"),
            Self::MissingTagName => write!(f, "missing tag name"),
            Self::InvalidCharacterReference => write!(f, "invalid character reference"),
            Self::InvalidDoctype => write!(f, "invalid doctype"),
        }
    }
}

impl std::error::Error for HtmlParseError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Data,
    TagOpen,
    EndTagOpen,
    TagName,
    BeforeAttributeName,
    AttributeName,
    AfterAttributeName,
    BeforeAttributeValue,
    AttributeValueDoubleQuoted,
    AttributeValueSingleQuoted,
    AttributeValueUnquoted,
    AfterAttributeValueQuoted,
    SelfClosingStartTag,
    MarkupDeclarationOpen,
    CommentStart,
    CommentStartDash,
    Comment,
    CommentEndDash,
    CommentEnd,
    Doctype,
    BeforeDoctypeName,
    DoctypeName,
}

/// A small HTML tokenizer.
///
/// # Examples
///
/// ```
/// use omoikane::html::{Token, Tokenizer};
///
/// let tokens = Tokenizer::new("<p class=test>Hello</p>").tokenize();
/// assert_eq!(tokens[0], Token::StartTag {
///     name: "p".to_string(),
///     attributes: vec![omoikane::html::Attribute::new("class", "test")],
///     self_closing: false,
/// });
/// ```
#[derive(Debug, Clone)]
pub struct Tokenizer<'a> {
    input: &'a str,
}

impl<'a> Tokenizer<'a> {
    /// Creates a tokenizer for the given input.
    pub fn new(input: &'a str) -> Self {
        Self { input }
    }

    /// Tokenizes the full input and returns the produced tokens.
    pub fn tokenize(&self) -> Vec<Token> {
        self.tokenize_with_errors().0
    }

    /// Tokenizes the full input and also returns recoverable parse errors.
    pub fn tokenize_with_errors(&self) -> (Vec<Token>, Vec<HtmlParseError>) {
        let chars: Vec<char> = self.input.chars().collect();
        let mut cursor = Cursor::new(chars);
        let mut tokens = Vec::new();
        let mut errors = Vec::new();
        let mut state = State::Data;

        let mut text_buffer = String::new();
        let mut current_tag_name = String::new();
        let mut current_end_tag_name = String::new();
        let mut current_attributes = Vec::new();
        let mut current_attr_name = String::new();
        let mut current_attr_value = String::new();
        let mut current_self_closing = false;
        let mut current_comment = String::new();
        let mut current_doctype_name = String::new();
        let mut current_doctype_force_quirks = false;

        while let Some(ch) = cursor.consume() {
            match state {
                State::Data => match ch {
                    '<' => {
                        flush_text(&mut text_buffer, &mut tokens);
                        state = State::TagOpen;
                    }
                    '&' => match consume_character_reference(&mut cursor) {
                        Ok(decoded) => text_buffer.push_str(&decoded),
                        Err(error) => {
                            errors.push(error);
                            text_buffer.push('&');
                        }
                    },
                    _ => text_buffer.push(ch),
                },
                State::TagOpen => match ch {
                    '/' => {
                        current_end_tag_name.clear();
                        state = State::EndTagOpen;
                    }
                    '!' => state = State::MarkupDeclarationOpen,
                    c if is_tag_name_start(c) => {
                        current_tag_name.clear();
                        current_tag_name.push(c.to_ascii_lowercase());
                        current_attributes.clear();
                        current_self_closing = false;
                        state = State::TagName;
                    }
                    _ => {
                        errors.push(HtmlParseError::MissingTagName);
                        text_buffer.push('<');
                        text_buffer.push(ch);
                        state = State::Data;
                    }
                },
                State::EndTagOpen => match ch {
                    c if is_tag_name_start(c) => {
                        current_end_tag_name.clear();
                        current_end_tag_name.push(c.to_ascii_lowercase());
                        state = State::TagName;
                    }
                    '>' => {
                        errors.push(HtmlParseError::MissingTagName);
                        state = State::Data;
                    }
                    _ => {
                        errors.push(HtmlParseError::MissingTagName);
                        text_buffer.push_str("</");
                        text_buffer.push(ch);
                        state = State::Data;
                    }
                },
                State::TagName => match ch {
                    c if is_html_whitespace(c) => {
                        state = State::BeforeAttributeName;
                    }
                    '/' => {
                        state = State::SelfClosingStartTag;
                    }
                    '>' => {
                        emit_tag(
                            &mut tokens,
                            &current_tag_name,
                            &current_end_tag_name,
                            &current_attributes,
                            current_self_closing,
                        );
                        current_tag_name.clear();
                        current_end_tag_name.clear();
                        current_attributes.clear();
                        current_self_closing = false;
                        state = State::Data;
                    }
                    c => {
                        if current_end_tag_name.is_empty() {
                            current_tag_name.push(c.to_ascii_lowercase());
                        } else {
                            current_end_tag_name.push(c.to_ascii_lowercase());
                        }
                    }
                },
                State::BeforeAttributeName => match ch {
                    c if is_html_whitespace(c) => {}
                    '/' => state = State::SelfClosingStartTag,
                    '>' => {
                        emit_tag(
                            &mut tokens,
                            &current_tag_name,
                            &current_end_tag_name,
                            &current_attributes,
                            current_self_closing,
                        );
                        current_tag_name.clear();
                        current_end_tag_name.clear();
                        current_attributes.clear();
                        current_self_closing = false;
                        state = State::Data;
                    }
                    _ => {
                        current_attr_name.clear();
                        current_attr_value.clear();
                        current_attr_name.push(ch.to_ascii_lowercase());
                        state = State::AttributeName;
                    }
                },
                State::AttributeName => match ch {
                    c if is_html_whitespace(c) => {
                        state = State::AfterAttributeName;
                    }
                    '=' => state = State::BeforeAttributeValue,
                    '/' => {
                        push_attribute(
                            &mut current_attributes,
                            &mut current_attr_name,
                            &mut current_attr_value,
                        );
                        state = State::SelfClosingStartTag;
                    }
                    '>' => {
                        push_attribute(
                            &mut current_attributes,
                            &mut current_attr_name,
                            &mut current_attr_value,
                        );
                        emit_tag(
                            &mut tokens,
                            &current_tag_name,
                            &current_end_tag_name,
                            &current_attributes,
                            current_self_closing,
                        );
                        current_tag_name.clear();
                        current_end_tag_name.clear();
                        current_attributes.clear();
                        current_self_closing = false;
                        state = State::Data;
                    }
                    c => current_attr_name.push(c.to_ascii_lowercase()),
                },
                State::AfterAttributeName => match ch {
                    c if is_html_whitespace(c) => {}
                    '=' => state = State::BeforeAttributeValue,
                    '/' => {
                        push_attribute(
                            &mut current_attributes,
                            &mut current_attr_name,
                            &mut current_attr_value,
                        );
                        state = State::SelfClosingStartTag;
                    }
                    '>' => {
                        push_attribute(
                            &mut current_attributes,
                            &mut current_attr_name,
                            &mut current_attr_value,
                        );
                        emit_tag(
                            &mut tokens,
                            &current_tag_name,
                            &current_end_tag_name,
                            &current_attributes,
                            current_self_closing,
                        );
                        current_tag_name.clear();
                        current_end_tag_name.clear();
                        current_attributes.clear();
                        current_self_closing = false;
                        state = State::Data;
                    }
                    _ => {
                        push_attribute(
                            &mut current_attributes,
                            &mut current_attr_name,
                            &mut current_attr_value,
                        );
                        current_attr_name.push(ch.to_ascii_lowercase());
                        state = State::AttributeName;
                    }
                },
                State::BeforeAttributeValue => match ch {
                    c if is_html_whitespace(c) => {}
                    '"' => state = State::AttributeValueDoubleQuoted,
                    '\'' => state = State::AttributeValueSingleQuoted,
                    '>' => {
                        push_attribute(
                            &mut current_attributes,
                            &mut current_attr_name,
                            &mut current_attr_value,
                        );
                        emit_tag(
                            &mut tokens,
                            &current_tag_name,
                            &current_end_tag_name,
                            &current_attributes,
                            current_self_closing,
                        );
                        current_tag_name.clear();
                        current_end_tag_name.clear();
                        current_attributes.clear();
                        current_self_closing = false;
                        state = State::Data;
                    }
                    _ => {
                        current_attr_value.push(ch);
                        state = State::AttributeValueUnquoted;
                    }
                },
                State::AttributeValueDoubleQuoted => match ch {
                    '"' => {
                        push_attribute(
                            &mut current_attributes,
                            &mut current_attr_name,
                            &mut current_attr_value,
                        );
                        state = State::AfterAttributeValueQuoted;
                    }
                    '&' => match consume_character_reference(&mut cursor) {
                        Ok(decoded) => current_attr_value.push_str(&decoded),
                        Err(error) => {
                            errors.push(error);
                            current_attr_value.push('&');
                        }
                    },
                    _ => current_attr_value.push(ch),
                },
                State::AttributeValueSingleQuoted => match ch {
                    '\'' => {
                        push_attribute(
                            &mut current_attributes,
                            &mut current_attr_name,
                            &mut current_attr_value,
                        );
                        state = State::AfterAttributeValueQuoted;
                    }
                    '&' => match consume_character_reference(&mut cursor) {
                        Ok(decoded) => current_attr_value.push_str(&decoded),
                        Err(error) => {
                            errors.push(error);
                            current_attr_value.push('&');
                        }
                    },
                    _ => current_attr_value.push(ch),
                },
                State::AttributeValueUnquoted => match ch {
                    c if is_html_whitespace(c) => {
                        push_attribute(
                            &mut current_attributes,
                            &mut current_attr_name,
                            &mut current_attr_value,
                        );
                        state = State::BeforeAttributeName;
                    }
                    '&' => match consume_character_reference(&mut cursor) {
                        Ok(decoded) => current_attr_value.push_str(&decoded),
                        Err(error) => {
                            errors.push(error);
                            current_attr_value.push('&');
                        }
                    },
                    '>' => {
                        push_attribute(
                            &mut current_attributes,
                            &mut current_attr_name,
                            &mut current_attr_value,
                        );
                        emit_tag(
                            &mut tokens,
                            &current_tag_name,
                            &current_end_tag_name,
                            &current_attributes,
                            current_self_closing,
                        );
                        current_tag_name.clear();
                        current_end_tag_name.clear();
                        current_attributes.clear();
                        current_self_closing = false;
                        state = State::Data;
                    }
                    _ => current_attr_value.push(ch),
                },
                State::AfterAttributeValueQuoted => match ch {
                    c if is_html_whitespace(c) => state = State::BeforeAttributeName,
                    '/' => state = State::SelfClosingStartTag,
                    '>' => {
                        emit_tag(
                            &mut tokens,
                            &current_tag_name,
                            &current_end_tag_name,
                            &current_attributes,
                            current_self_closing,
                        );
                        current_tag_name.clear();
                        current_end_tag_name.clear();
                        current_attributes.clear();
                        current_self_closing = false;
                        state = State::Data;
                    }
                    _ => {
                        errors.push(HtmlParseError::UnexpectedEof);
                        current_attr_name.clear();
                        current_attr_value.clear();
                        state = State::BeforeAttributeName;
                        cursor.reconsume();
                    }
                },
                State::SelfClosingStartTag => match ch {
                    '>' => {
                        current_self_closing = true;
                        emit_tag(
                            &mut tokens,
                            &current_tag_name,
                            &current_end_tag_name,
                            &current_attributes,
                            current_self_closing,
                        );
                        current_tag_name.clear();
                        current_end_tag_name.clear();
                        current_attributes.clear();
                        current_self_closing = false;
                        state = State::Data;
                    }
                    _ => {
                        state = State::BeforeAttributeName;
                        cursor.reconsume();
                    }
                },
                State::MarkupDeclarationOpen => {
                    if ch == '-' && cursor.peek() == Some('-') {
                        let _ = cursor.consume();
                        current_comment.clear();
                        state = State::CommentStart;
                    } else if ch.eq_ignore_ascii_case(&'d') {
                        let mut lookahead = String::from(ch);
                        let mut doctype_candidate_complete = true;
                        for _ in 0..6 {
                            if let Some(next) = cursor.consume() {
                                lookahead.push(next);
                            } else {
                                errors.push(HtmlParseError::InvalidDoctype);
                                doctype_candidate_complete = false;
                                break;
                            }
                        }

                        if doctype_candidate_complete && lookahead.eq_ignore_ascii_case("doctype") {
                            current_doctype_name.clear();
                            current_doctype_force_quirks = false;
                            state = State::Doctype;
                        } else {
                            if doctype_candidate_complete {
                                errors.push(HtmlParseError::InvalidDoctype);
                            }
                            current_comment = lookahead;
                            state = State::Comment;
                        }
                    } else {
                        current_comment.clear();
                        current_comment.push(ch);
                        state = State::Comment;
                    }
                }
                State::CommentStart => match ch {
                    '-' => state = State::CommentStartDash,
                    '>' => {
                        tokens.push(Token::Comment(String::new()));
                        state = State::Data;
                    }
                    _ => {
                        current_comment.push(ch);
                        state = State::Comment;
                    }
                },
                State::CommentStartDash => match ch {
                    '-' => state = State::CommentEnd,
                    '>' => {
                        tokens.push(Token::Comment(String::new()));
                        state = State::Data;
                    }
                    _ => {
                        current_comment.push('-');
                        current_comment.push(ch);
                        state = State::Comment;
                    }
                },
                State::Comment => match ch {
                    '-' => state = State::CommentEndDash,
                    _ => current_comment.push(ch),
                },
                State::CommentEndDash => match ch {
                    '-' => state = State::CommentEnd,
                    _ => {
                        current_comment.push('-');
                        current_comment.push(ch);
                        state = State::Comment;
                    }
                },
                State::CommentEnd => match ch {
                    '>' => {
                        tokens.push(Token::Comment(std::mem::take(&mut current_comment)));
                        state = State::Data;
                    }
                    '-' => current_comment.push('-'),
                    _ => {
                        current_comment.push_str("--");
                        current_comment.push(ch);
                        state = State::Comment;
                    }
                },
                State::Doctype => match ch {
                    c if is_html_whitespace(c) => state = State::BeforeDoctypeName,
                    '>' => {
                        errors.push(HtmlParseError::InvalidDoctype);
                        tokens.push(Token::Doctype(DoctypeToken {
                            name: None,
                            force_quirks: true,
                        }));
                        state = State::Data;
                    }
                    _ => {
                        errors.push(HtmlParseError::InvalidDoctype);
                        current_doctype_name.push(ch.to_ascii_lowercase());
                        state = State::DoctypeName;
                    }
                },
                State::BeforeDoctypeName => match ch {
                    c if is_html_whitespace(c) => {}
                    '>' => {
                        errors.push(HtmlParseError::InvalidDoctype);
                        tokens.push(Token::Doctype(DoctypeToken {
                            name: None,
                            force_quirks: true,
                        }));
                        state = State::Data;
                    }
                    _ => {
                        current_doctype_name.clear();
                        current_doctype_name.push(ch.to_ascii_lowercase());
                        state = State::DoctypeName;
                    }
                },
                State::DoctypeName => match ch {
                    c if is_html_whitespace(c) => {
                        current_doctype_force_quirks = true;
                    }
                    '>' => {
                        tokens.push(Token::Doctype(DoctypeToken {
                            name: Some(std::mem::take(&mut current_doctype_name)),
                            force_quirks: current_doctype_force_quirks,
                        }));
                        current_doctype_force_quirks = false;
                        state = State::Data;
                    }
                    _ => current_doctype_name.push(ch.to_ascii_lowercase()),
                },
            }
        }

        if !text_buffer.is_empty() {
            tokens.push(Token::Character(text_buffer));
        }

        match state {
            State::Comment
            | State::CommentEnd
            | State::CommentEndDash
            | State::CommentStart
            | State::CommentStartDash
            | State::TagOpen
            | State::EndTagOpen
            | State::TagName
            | State::BeforeAttributeName
            | State::AttributeName
            | State::AfterAttributeName
            | State::BeforeAttributeValue
            | State::AttributeValueDoubleQuoted
            | State::AttributeValueSingleQuoted
            | State::AttributeValueUnquoted
            | State::AfterAttributeValueQuoted
            | State::SelfClosingStartTag
            | State::MarkupDeclarationOpen
            | State::Doctype
            | State::BeforeDoctypeName
            | State::DoctypeName => errors.push(HtmlParseError::UnexpectedEof),
            State::Data => {}
        }

        tokens.push(Token::Eof);
        (tokens, errors)
    }
}

#[derive(Debug, Clone)]
struct Cursor {
    chars: Vec<char>,
    index: usize,
    can_reconsume: bool,
}

impl Cursor {
    fn new(chars: Vec<char>) -> Self {
        Self {
            chars,
            index: 0,
            can_reconsume: false,
        }
    }

    fn consume(&mut self) -> Option<char> {
        let ch = self.chars.get(self.index).copied()?;
        self.index += 1;
        self.can_reconsume = true;
        Some(ch)
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.index).copied()
    }

    fn reconsume(&mut self) {
        if self.can_reconsume && self.index > 0 {
            self.index -= 1;
            self.can_reconsume = false;
        }
    }
}

fn is_tag_name_start(ch: char) -> bool {
    ch.is_ascii_alphabetic()
}

fn is_html_whitespace(ch: char) -> bool {
    matches!(ch, '\t' | '\n' | '\x0C' | '\r' | ' ')
}

fn flush_text(buffer: &mut String, tokens: &mut Vec<Token>) {
    if !buffer.is_empty() {
        tokens.push(Token::Character(std::mem::take(buffer)));
    }
}

fn push_attribute(attributes: &mut Vec<Attribute>, name: &mut String, value: &mut String) {
    if !name.is_empty() {
        attributes.push(Attribute::new(std::mem::take(name), std::mem::take(value)));
    }
}

fn emit_tag(
    tokens: &mut Vec<Token>,
    current_tag_name: &str,
    current_end_tag_name: &str,
    current_attributes: &[Attribute],
    current_self_closing: bool,
) {
    if !current_end_tag_name.is_empty() {
        tokens.push(Token::EndTag {
            name: current_end_tag_name.to_string(),
        });
        return;
    }

    if !current_tag_name.is_empty() {
        tokens.push(Token::StartTag {
            name: current_tag_name.to_string(),
            attributes: current_attributes.to_vec(),
            self_closing: current_self_closing,
        });
    }
}

fn consume_character_reference(cursor: &mut Cursor) -> Result<String, HtmlParseError> {
    let mut entity = String::new();

    while let Some(ch) = cursor.peek() {
        if ch == ';' {
            let _ = cursor.consume();
            break;
        }

        if !(ch.is_ascii_alphanumeric() || ch == '#') {
            return Err(HtmlParseError::InvalidCharacterReference);
        }

        entity.push(cursor.consume().expect("peeked char must be consumable"));
    }

    if entity.is_empty() {
        return Err(HtmlParseError::InvalidCharacterReference);
    }

    match entity.as_str() {
        "amp" => Ok("&".to_string()),
        "lt" => Ok("<".to_string()),
        "gt" => Ok(">".to_string()),
        "quot" => Ok("\"".to_string()),
        "apos" => Ok("'".to_string()),
        _ => {
            if let Some(rest) = entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
            {
                let value = u32::from_str_radix(rest, 16)
                    .map_err(|_| HtmlParseError::InvalidCharacterReference)?;
                char::from_u32(value)
                    .map(|ch| ch.to_string())
                    .ok_or(HtmlParseError::InvalidCharacterReference)
            } else if let Some(rest) = entity.strip_prefix('#') {
                let value = rest
                    .parse::<u32>()
                    .map_err(|_| HtmlParseError::InvalidCharacterReference)?;
                char::from_u32(value)
                    .map(|ch| ch.to_string())
                    .ok_or(HtmlParseError::InvalidCharacterReference)
            } else {
                Err(HtmlParseError::InvalidCharacterReference)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_plain_text() {
        let tokens = Tokenizer::new("hello").tokenize();
        assert_eq!(
            tokens,
            vec![Token::Character("hello".to_string()), Token::Eof]
        );
    }

    #[test]
    fn tokenizes_start_and_end_tags() {
        let tokens = Tokenizer::new("<div>hello</div>").tokenize();
        assert_eq!(
            tokens,
            vec![
                Token::StartTag {
                    name: "div".to_string(),
                    attributes: vec![],
                    self_closing: false,
                },
                Token::Character("hello".to_string()),
                Token::EndTag {
                    name: "div".to_string(),
                },
                Token::Eof,
            ]
        );
    }

    #[test]
    fn tokenizes_attributes_and_self_closing_tag() {
        let tokens = Tokenizer::new(r#"<img src="a.png" alt=test disabled />"#).tokenize();
        assert_eq!(
            tokens,
            vec![
                Token::StartTag {
                    name: "img".to_string(),
                    attributes: vec![
                        Attribute::new("src", "a.png"),
                        Attribute::new("alt", "test"),
                        Attribute::new("disabled", ""),
                    ],
                    self_closing: true,
                },
                Token::Eof,
            ]
        );
    }

    #[test]
    fn tokenizes_comment() {
        let tokens = Tokenizer::new("<!--note-->").tokenize();
        assert_eq!(tokens, vec![Token::Comment("note".to_string()), Token::Eof]);
    }

    #[test]
    fn tokenizes_doctype() {
        let tokens = Tokenizer::new("<!DOCTYPE html>").tokenize();
        assert_eq!(
            tokens,
            vec![
                Token::Doctype(DoctypeToken {
                    name: Some("html".to_string()),
                    force_quirks: false,
                }),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn decodes_character_references_in_text() {
        let tokens = Tokenizer::new("&lt;p&gt;&#x41;&amp;").tokenize();
        assert_eq!(
            tokens,
            vec![Token::Character("<p>A&".to_string()), Token::Eof]
        );
    }

    #[test]
    fn decodes_character_references_in_attributes() {
        let tokens = Tokenizer::new(r#"<a title="Tom &amp; Jerry">"#).tokenize();
        assert_eq!(
            tokens,
            vec![
                Token::StartTag {
                    name: "a".to_string(),
                    attributes: vec![Attribute::new("title", "Tom & Jerry")],
                    self_closing: false,
                },
                Token::Eof,
            ]
        );
    }

    #[test]
    fn reports_invalid_character_reference() {
        let (_, errors) = Tokenizer::new("&bogus;").tokenize_with_errors();
        assert_eq!(errors, vec![HtmlParseError::InvalidCharacterReference]);
    }

    #[test]
    fn reports_missing_tag_name() {
        let (_, errors) = Tokenizer::new("<>").tokenize_with_errors();
        assert_eq!(errors, vec![HtmlParseError::MissingTagName]);
    }

    #[test]
    fn reports_unexpected_eof() {
        let (_, errors) = Tokenizer::new("<div").tokenize_with_errors();
        assert_eq!(errors, vec![HtmlParseError::UnexpectedEof]);
    }
}
