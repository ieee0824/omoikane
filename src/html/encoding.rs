use encoding_rs::Encoding;

pub(crate) fn decode_html_response(response: &crate::http::HttpResponse) -> String {
    let body = response.body();
    let charset = response
        .header("content-type")
        .and_then(parse_charset_from_content_type)
        .or_else(|| detect_charset_from_html_meta(body));

    if let Some(label) = charset.as_deref() {
        if let Some(encoding) = Encoding::for_label(label.as_bytes()) {
            let (decoded, _, _) = encoding.decode(body);
            return decoded.into_owned();
        }
    }

    String::from_utf8_lossy(body).to_string()
}

pub(crate) fn detect_charset_from_html_meta(body: &[u8]) -> Option<String> {
    let head = String::from_utf8_lossy(&body[..body.len().min(8192)]).to_string();
    let lower = head.to_ascii_lowercase();
    let mut cursor = 0usize;
    while let Some(relative) = lower[cursor..].find("<meta") {
        let start = cursor + relative;
        let Some(end_rel) = lower[start..].find('>') else {
            break;
        };
        let end = start + end_rel + 1;
        let tag = &head[start..end];
        if let Some(attributes) = parse_html_attributes(tag) {
            if let Some(charset) = attributes
                .get("charset")
                .map(|value| value.trim().to_ascii_lowercase())
                .filter(|value| !value.is_empty())
            {
                return Some(charset);
            }
            let has_content_type_equiv = attributes
                .get("http-equiv")
                .map(|value| value.trim().eq_ignore_ascii_case("content-type"))
                .unwrap_or(false);
            if has_content_type_equiv {
                if let Some(content) = attributes.get("content") {
                    if let Some(charset) = parse_charset_from_content_type(content) {
                        return Some(charset);
                    }
                }
            }
        }
        cursor = end;
    }
    None
}

fn parse_charset_from_content_type(content_type: &str) -> Option<String> {
    content_type.split(';').skip(1).find_map(|part| {
        let (name, value) = part.split_once('=')?;
        if !name.trim().eq_ignore_ascii_case("charset") {
            return None;
        }
        let value = value.trim().trim_matches('"').trim_matches('\'');
        if value.is_empty() {
            None
        } else {
            Some(value.to_ascii_lowercase())
        }
    })
}

fn parse_html_attributes(tag: &str) -> Option<std::collections::BTreeMap<String, String>> {
    let mut attributes = std::collections::BTreeMap::new();
    let open = tag.find('<')?;
    let close = tag.rfind('>')?;
    if close <= open {
        return None;
    }
    let mut chars = tag[open + 1..close].chars().peekable();

    while let Some(ch) = chars.peek() {
        if ch.is_ascii_whitespace() {
            chars.next();
        } else {
            break;
        }
    }
    while let Some(ch) = chars.peek() {
        if ch.is_ascii_whitespace() {
            break;
        }
        chars.next();
    }

    loop {
        while let Some(ch) = chars.peek() {
            if ch.is_ascii_whitespace() {
                chars.next();
            } else {
                break;
            }
        }
        let mut name = String::new();
        while let Some(ch) = chars.peek() {
            if ch.is_ascii_whitespace() || *ch == '=' || *ch == '/' {
                break;
            }
            name.push(*ch);
            chars.next();
        }
        if name.is_empty() {
            break;
        }
        while let Some(ch) = chars.peek() {
            if ch.is_ascii_whitespace() {
                chars.next();
            } else {
                break;
            }
        }
        let mut value = String::new();
        if chars.peek() == Some(&'=') {
            chars.next();
            while let Some(ch) = chars.peek() {
                if ch.is_ascii_whitespace() {
                    chars.next();
                } else {
                    break;
                }
            }
            if let Some(quote) = chars.peek().copied().filter(|c| *c == '"' || *c == '\'') {
                chars.next();
                while let Some(ch) = chars.peek() {
                    if *ch == quote {
                        chars.next();
                        break;
                    }
                    value.push(*ch);
                    chars.next();
                }
            } else {
                while let Some(ch) = chars.peek() {
                    if ch.is_ascii_whitespace() || *ch == '/' {
                        break;
                    }
                    value.push(*ch);
                    chars.next();
                }
            }
        }
        attributes.insert(name.to_ascii_lowercase(), value);
    }

    Some(attributes)
}

