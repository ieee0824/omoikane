//! CSS tokenizer.

use super::{CssParseError, CssToken};

/// Tokenizes a CSS string.
pub fn tokenize(input: &str) -> Result<Vec<CssToken>, CssParseError> {
    let chars: Vec<char> = input.chars().collect();
    let mut index = 0;
    let mut tokens = Vec::new();

    while let Some(&ch) = chars.get(index) {
        match ch {
            c if c.is_ascii_whitespace() => {
                while chars.get(index).is_some_and(|c| c.is_ascii_whitespace()) {
                    index += 1;
                }
                tokens.push(CssToken::Whitespace);
            }
            '/' if chars.get(index + 1) == Some(&'*') => {
                index += 2;
                while index + 1 < chars.len() && !(chars[index] == '*' && chars[index + 1] == '/') {
                    index += 1;
                }
                if index + 1 >= chars.len() {
                    return Err(CssParseError::UnexpectedEndOfInput);
                }
                index += 2;
            }
            '@' => {
                index += 1;
                let ident = consume_ident(&chars, &mut index);
                tokens.push(CssToken::AtKeyword(ident));
            }
            '#' => {
                index += 1;
                let ident = consume_ident(&chars, &mut index);
                tokens.push(CssToken::Hash(ident));
            }
            '"' | '\'' => {
                let string = consume_string(&chars, &mut index, ch)?;
                tokens.push(CssToken::String(string));
            }
            _ if is_number_start(&chars, index) => {
                let number = consume_number(&chars, &mut index)?;
                if chars.get(index) == Some(&'%') {
                    index += 1;
                    tokens.push(CssToken::Percentage(number));
                } else if chars.get(index).is_some_and(|c| is_ident_start(*c)) {
                    let unit = consume_ident(&chars, &mut index);
                    tokens.push(CssToken::Dimension(number, unit));
                } else {
                    tokens.push(CssToken::Number(number));
                }
            }
            c if is_ident_start(c) || c == '-' || c == '\\' => {
                let ident = consume_ident(&chars, &mut index);
                tokens.push(CssToken::Ident(ident));
            }
            ':' => {
                index += 1;
                tokens.push(CssToken::Colon);
            }
            ';' => {
                index += 1;
                tokens.push(CssToken::Semicolon);
            }
            ',' => {
                index += 1;
                tokens.push(CssToken::Comma);
            }
            '{' => {
                index += 1;
                tokens.push(CssToken::CurlyOpen);
            }
            '}' => {
                index += 1;
                tokens.push(CssToken::CurlyClose);
            }
            '(' => {
                index += 1;
                tokens.push(CssToken::ParenOpen);
            }
            ')' => {
                index += 1;
                tokens.push(CssToken::ParenClose);
            }
            '[' => {
                index += 1;
                tokens.push(CssToken::BracketOpen);
            }
            ']' => {
                index += 1;
                tokens.push(CssToken::BracketClose);
            }
            _ => {
                index += 1;
                tokens.push(CssToken::Delim(ch));
            }
        }
    }

    Ok(tokens)
}

pub(super) fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_' || ch == '-'
}

pub(super) fn is_ident_char(ch: char) -> bool {
    is_ident_start(ch) || ch.is_ascii_digit()
}

fn is_number_start(chars: &[char], index: usize) -> bool {
    chars.get(index).is_some_and(|c| c.is_ascii_digit())
        || (chars.get(index) == Some(&'.')
            && chars.get(index + 1).is_some_and(|c| c.is_ascii_digit()))
        || ((chars.get(index) == Some(&'+') || chars.get(index) == Some(&'-'))
            && chars
                .get(index + 1)
                .is_some_and(|c| c.is_ascii_digit() || *c == '.'))
}

fn consume_css_escape(chars: &[char], index: &mut usize) -> Option<char> {
    // CSS 2.1 section 4.1.3: backslash escapes
    // \<hex>{1,6} followed by optional whitespace -> Unicode code point
    // \<non-hex> -> literal character
    let start = *index;
    if chars.get(start) != Some(&'\\') {
        return None;
    }
    let next = chars.get(start + 1)?;
    if next.is_ascii_hexdigit() {
        let mut hex = String::new();
        let mut i = start + 1;
        while i < chars.len() && hex.len() < 6 && chars[i].is_ascii_hexdigit() {
            hex.push(chars[i]);
            i += 1;
        }
        // Consume optional trailing whitespace
        if i < chars.len() && chars[i].is_ascii_whitespace() {
            i += 1;
        }
        *index = i;
        u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32)
    } else {
        *index = start + 2;
        Some(*next)
    }
}

pub(super) fn consume_ident(chars: &[char], index: &mut usize) -> String {
    let mut ident = String::new();
    while let Some(&ch) = chars.get(*index) {
        if ch == '\\' {
            if let Some(escaped) = consume_css_escape(chars, index) {
                ident.push(escaped);
            } else {
                *index += 1;
            }
        } else if is_ident_char(ch) {
            ident.push(ch);
            *index += 1;
        } else {
            break;
        }
    }
    ident
}

pub(super) fn consume_string(chars: &[char], index: &mut usize, quote: char) -> Result<String, CssParseError> {
    *index += 1;
    let mut value = String::new();
    while let Some(&ch) = chars.get(*index) {
        if ch == quote {
            *index += 1;
            return Ok(value);
        }
        if ch == '\\' {
            if let Some(escaped) = consume_css_escape(chars, index) {
                value.push(escaped);
            } else {
                *index += 1;
            }
        } else {
            *index += 1;
            value.push(ch);
        }
    }
    Err(CssParseError::UnexpectedEndOfInput)
}

pub(super) fn consume_number(chars: &[char], index: &mut usize) -> Result<f32, CssParseError> {
    let mut value = String::new();
    if let Some(sign @ ('+' | '-')) = chars.get(*index).copied() {
        value.push(sign);
        *index += 1;
    }
    if chars.get(*index) == Some(&'.') {
        value.push('.');
        *index += 1;
    }
    while let Some(&ch) = chars.get(*index) {
        if ch.is_ascii_digit() || ch == '.' {
            value.push(ch);
            *index += 1;
        } else {
            break;
        }
    }
    value
        .parse::<f32>()
        .map_err(|_| CssParseError::InvalidNumber)
}

pub(super) fn render_tokens(tokens: &[CssToken]) -> String {
    let mut rendered = String::new();
    for token in tokens {
        match token {
            CssToken::Ident(value) => rendered.push_str(value),
            CssToken::AtKeyword(value) => {
                rendered.push('@');
                rendered.push_str(value);
            }
            CssToken::Hash(value) => {
                rendered.push('#');
                rendered.push_str(value);
            }
            CssToken::String(value) => {
                rendered.push('"');
                rendered.push_str(value);
                rendered.push('"');
            }
            CssToken::Number(value) => rendered.push_str(&trimmed_number(*value)),
            CssToken::Percentage(value) => {
                rendered.push_str(&trimmed_number(*value));
                rendered.push('%');
            }
            CssToken::Dimension(value, unit) => {
                rendered.push_str(&trimmed_number(*value));
                rendered.push_str(unit);
            }
            CssToken::Colon => rendered.push(':'),
            CssToken::Semicolon => rendered.push(';'),
            CssToken::Comma => rendered.push(','),
            CssToken::CurlyOpen => rendered.push('{'),
            CssToken::CurlyClose => rendered.push('}'),
            CssToken::ParenOpen => rendered.push('('),
            CssToken::ParenClose => rendered.push(')'),
            CssToken::BracketOpen => rendered.push('['),
            CssToken::BracketClose => rendered.push(']'),
            CssToken::Delim(ch) => rendered.push(*ch),
            CssToken::Whitespace => rendered.push(' '),
        }
    }
    rendered
}

pub(super) fn trimmed_number(value: f32) -> String {
    let mut text = value.to_string();
    if text.ends_with(".0") {
        text.truncate(text.len() - 2);
    }
    text
}
