//! Remove CSS comments before parsing query-list punctuation.

/// Keeps token boundaries rather than joining identifiers or dimensions across
/// a comment. Quoted text and escaped characters remain literal.
pub(super) fn normalize(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut quote = None;
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            output.push(ch);
            if let Some(escaped) = chars.next() {
                output.push(escaped);
            }
            continue;
        }
        if let Some(delimiter) = quote {
            output.push(ch);
            if ch == delimiter {
                quote = None;
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
            output.push(ch);
        } else if ch == '/' && chars.peek() == Some(&'*') {
            loop {
                chars.next(); // opening '*', or '/' of the next comment
                while let Some(comment_char) = chars.next() {
                    if comment_char == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        break;
                    }
                }
                if !chars.clone().take(2).eq(['/', '*']) {
                    break;
                }
                chars.next(); // next opening '/'; loop consumes '*'
            }
            // MQ4 forbids whitespace between comparison delimiters, but a
            // consumed comment emits no whitespace token. Keep real spaces.
            let comparison_pair =
                matches!(output.chars().next_back(), Some('<' | '>')) && chars.peek() == Some(&'=');
            if !comparison_pair {
                output.push(' ');
            }
        } else {
            output.push(ch);
        }
    }
    output
}
