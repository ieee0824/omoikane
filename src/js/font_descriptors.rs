//! Font descriptor declarations retain URL payloads and quoted delimiters.

/// Splits a stylesheet into original top-level rule source slices.
pub(super) fn rule_sources(input: &str) -> Vec<String> {
    let bytes = input.as_bytes();
    let mut result = Vec::new();
    let mut start = 0;
    let mut index = 0;
    let mut quote = None;
    let mut brace_depth = 0usize;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut comment = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if comment {
            if byte == b'*' && bytes.get(index + 1) == Some(&b'/') {
                comment = false;
                index += 1;
            }
        } else if byte == b'\\' {
            index += 1;
        } else if let Some(delimiter) = quote {
            if byte == delimiter {
                quote = None;
            }
        } else if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            comment = true;
            index += 1;
        } else {
            match byte {
                b'\'' | b'"' => quote = Some(byte),
                b'(' => paren_depth += 1,
                b')' => paren_depth = paren_depth.saturating_sub(1),
                b'[' => bracket_depth += 1,
                b']' => bracket_depth = bracket_depth.saturating_sub(1),
                b'{' if paren_depth == 0 && bracket_depth == 0 => brace_depth += 1,
                b'}' if paren_depth == 0 && bracket_depth == 0 => {
                    brace_depth = brace_depth.saturating_sub(1);
                    if brace_depth == 0 {
                        push_rule(&input[start..=index], &mut result);
                        start = index + 1;
                    }
                }
                b';' if brace_depth == 0 && paren_depth == 0 && bracket_depth == 0 => {
                    push_rule(&input[start..=index], &mut result);
                    start = index + 1;
                }
                _ => {}
            }
        }
        index += 1;
    }
    push_rule(&input[start..], &mut result);
    result
}

fn push_rule(input: &str, result: &mut Vec<String>) {
    let input = input.trim();
    if !input.is_empty() {
        result.push(input.to_owned());
    }
}

/// Splits a descriptor block at top-level semicolons, never inside a URL,
/// quoted value, escape or comment. Values remain CSS source for the descriptor
/// parser, rather than being converted through numeric CSS values.
pub(super) fn declarations(input: &str) -> Vec<(String, String)> {
    let bytes = input.as_bytes();
    let mut result = Vec::new();
    let mut start = 0;
    let mut index = 0;
    let mut quote = None;
    let mut depth = 0usize;
    let mut comment = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if comment {
            if byte == b'*' && bytes.get(index + 1) == Some(&b'/') {
                comment = false;
                index += 1;
            }
        } else if byte == b'\\' {
            index += 1;
        } else if let Some(delimiter) = quote {
            if byte == delimiter {
                quote = None;
            }
        } else if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            comment = true;
            index += 1;
        } else {
            match byte {
                b'\'' | b'"' => quote = Some(byte),
                b'(' | b'[' => depth += 1,
                b')' | b']' => depth = depth.saturating_sub(1),
                b';' if depth == 0 => {
                    append(&input[start..index], &mut result);
                    start = index + 1;
                }
                _ => {}
            }
        }
        index += 1;
    }
    if quote.is_none() && depth == 0 && !comment {
        append(&input[start..], &mut result);
    }
    result
}

fn append(input: &str, result: &mut Vec<(String, String)>) {
    let mut comment = false;
    let mut index = 0;
    let bytes = input.as_bytes();
    let mut colon = None;
    while index < bytes.len() {
        if comment {
            if bytes[index] == b'*' && bytes.get(index + 1) == Some(&b'/') {
                comment = false;
                index += 1;
            }
        } else if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'*') {
            comment = true;
            index += 1;
        } else if bytes[index] == b':' {
            colon = Some(index);
            break;
        }
        index += 1;
    }
    let Some(colon) = colon else {
        return;
    };
    let (name, value) = (&input[..colon], &input[colon + 1..]);
    let Ok(tokens) = crate::css::tokenize(name) else {
        return;
    };
    let tokens: Vec<_> = tokens
        .into_iter()
        .filter(|token| !matches!(token, crate::css::CssToken::Whitespace))
        .collect();
    let [crate::css::CssToken::Ident(name)] = tokens.as_slice() else {
        return;
    };
    let value = value.trim();
    if !value.is_empty() && crate::css::tokenize(value).is_ok() {
        result.push((name.to_ascii_lowercase(), value.to_owned()));
    }
}

#[cfg(test)]
mod tests {
    use super::{declarations, rule_sources};

    #[test]
    fn retains_urls_and_top_level_rule_boundaries() {
        let css = "@import url('a;b.css'); @font-face{font-family:A;src:url(data:font/ttf;base64,A;B)} /* } */ p{content:'};'}";
        assert_eq!(
            rule_sources(css),
            [
                "@import url('a;b.css');",
                "@font-face{font-family:A;src:url(data:font/ttf;base64,A;B)}",
                "/* } */ p{content:'};'}",
            ]
        );
    }

    #[test]
    fn retains_data_payload_quotes_and_unicode_range_source() {
        let declarations = declarations(
            "font-family:'A;B'; src:url(data:font/ttf;base64,AAAA00000123);unicode-range:U+0000-007F; font-weight:700",
        );
        assert_eq!(
            declarations,
            [
                ("font-family".into(), "'A;B'".into()),
                (
                    "src".into(),
                    "url(data:font/ttf;base64,AAAA00000123)".into()
                ),
                ("unicode-range".into(), "U+0000-007F".into()),
                ("font-weight".into(), "700".into()),
            ]
        );
    }

    #[test]
    fn comments_and_escaped_delimiters_do_not_split_declarations() {
        let declarations = declarations(
            r#"/* ; : */font-family:"A\";B"; src:url(font\;one.ttf);broken; font-style:italic"#,
        );
        assert_eq!(declarations.len(), 3);
        assert_eq!(declarations[0].1, r#""A\";B""#);
        assert_eq!(declarations[1].1, r"url(font\;one.ttf)");
        assert_eq!(declarations[2].1, "italic");
    }
}
