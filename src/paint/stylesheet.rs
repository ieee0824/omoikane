//! Author stylesheet extraction, @import resolution, and forgiving parsing.

use std::collections::HashSet;
use std::sync::Arc;

use crate::css::{Stylesheet, extract_font_face_rules, parse_stylesheet};
use crate::dom::{Node, NodeHandle, NodeType};
use crate::font::Font;
use crate::http::url::resolve_url;

use super::image::parse_data_uri;
use super::{DataUri, PaintError};

pub(crate) fn extract_author_stylesheets(
    document: &NodeHandle,
    base_url: Option<&crate::http::Url>,
) -> Result<Vec<String>, PaintError> {
    // Compute effective base URL considering <base> element
    let effective_base = extract_document_base_url(document, base_url);

    let mut stylesheets = Vec::new();
    let mut client = effective_base.as_ref().map(|_| crate::http::Client::new());
    collect_author_stylesheets(
        document,
        &mut stylesheets,
        effective_base.as_ref(),
        &mut client,
    )?;
    Ok(stylesheets)
}

const MAX_EXTERNAL_STYLESHEET_BYTES: usize = 4 * 1024 * 1024; // 4 MiB limit
const MAX_IMPORT_DEPTH: usize = 5;

pub(crate) fn collect_author_stylesheets(
    node: &NodeHandle,
    out: &mut Vec<String>,
    base_url: Option<&crate::http::Url>,
    client: &mut Option<crate::http::Client>,
) -> Result<(), PaintError> {
    if node.node_type() == NodeType::Element {
        // Rendering runs with scripting enabled, so fallback content inside
        // <noscript> must not contribute author stylesheets. In particular,
        // X includes a rule there that hides both #placeholder and #react-root.
        if node
            .tag_name()
            .as_deref()
            .is_some_and(|tag| tag.eq_ignore_ascii_case("noscript"))
        {
            return Ok(());
        }

        match node.tag_name().as_deref() {
            Some("style") => {
                let css = collect_text_contents(node);
                if !css.trim().is_empty() {
                    let media = node
                        .attributes()
                        .and_then(|attributes| attributes.get("media").cloned());
                    let start = out.len();
                    let mut active_import_urls = HashSet::new();
                    collect_stylesheet_with_imports(
                        css,
                        base_url,
                        base_url,
                        out,
                        client,
                        0,
                        &mut active_import_urls,
                    )?;
                    wrap_stylesheets_in_media(&mut out[start..], media.as_deref());
                }
            }
            Some("link") => {
                let attributes = node.attributes().unwrap_or_default();
                let rel = attributes.get("rel").cloned().unwrap_or_default();
                let href = attributes
                    .get("href")
                    .cloned()
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let media = attributes.get("media").map(|s| s.as_str());

                if rel
                    .split_whitespace()
                    .any(|token| token.eq_ignore_ascii_case("stylesheet"))
                    && !href.is_empty()
                    && matches_screen_media(media)
                {
                    let start = out.len();
                    if href.starts_with("data:text/css") {
                        let mut active_import_urls = HashSet::new();
                        match parse_data_uri(&href)? {
                            DataUri::Text { data, .. } => collect_stylesheet_with_imports(
                                data,
                                None,
                                base_url,
                                out,
                                client,
                                0,
                                &mut active_import_urls,
                            )?,
                            DataUri::Binary { data, .. } => collect_stylesheet_with_imports(
                                String::from_utf8_lossy(&data).into_owned(),
                                None,
                                base_url,
                                out,
                                client,
                                0,
                                &mut active_import_urls,
                            )?,
                        }
                    } else if let Some(base) = base_url
                        && let Some((css, resolved)) =
                            fetch_relative_stylesheet(base, &href, client, base_url)
                    {
                        let mut active_import_urls = HashSet::new();
                        collect_stylesheet_with_imports(
                            css,
                            Some(&resolved),
                            base_url,
                            out,
                            client,
                            0,
                            &mut active_import_urls,
                        )?;
                    }
                    wrap_stylesheets_in_media(&mut out[start..], media);
                }
            }
            _ => {}
        }
    }

    for child in node.child_nodes() {
        collect_author_stylesheets(&child, out, base_url, client)?;
    }

    Ok(())
}

fn wrap_stylesheets_in_media(stylesheets: &mut [String], media: Option<&str>) {
    let Some(media) = media.map(str::trim).filter(|media| !media.is_empty()) else {
        return;
    };
    for stylesheet in stylesheets {
        *stylesheet = format!("@media {media} {{\n{stylesheet}\n}}");
    }
}

pub(crate) fn collect_stylesheet_with_imports(
    css: String,
    stylesheet_url: Option<&crate::http::Url>,
    document_base: Option<&crate::http::Url>,
    out: &mut Vec<String>,
    client: &mut Option<crate::http::Client>,
    depth: usize,
    active_import_urls: &mut HashSet<String>,
) -> Result<(), PaintError> {
    if depth < MAX_IMPORT_DEPTH {
        let import_base = stylesheet_url.or(document_base);
        let directives = extract_import_directives(&css);
        if !directives.is_empty() {
            let chars: Vec<char> = css.chars().collect();
            let mut cursor = 0usize;
            for directive in directives {
                push_stylesheet_chunk(&chars[cursor..directive.start], stylesheet_url, out);
                cursor = directive.end;

                let mut imported = Vec::new();
                let supports_matches = directive
                    .supports
                    .as_deref()
                    .is_none_or(crate::css::supports_condition_matches);
                if supports_matches
                    && let Some(base) = import_base
                    && let Some(import_url) =
                        resolve_relative_stylesheet_url(base, &directive.href, document_base)
                {
                    let import_url_string = import_url.to_string();
                    if active_import_urls.insert(import_url_string.clone()) {
                        if let Some(import_css) =
                            fetch_stylesheet_by_url(&import_url, client, document_base)
                        {
                            collect_stylesheet_with_imports(
                                import_css,
                                Some(&import_url),
                                document_base,
                                &mut imported,
                                client,
                                depth + 1,
                                active_import_urls,
                            )?;
                        }
                        active_import_urls.remove(&import_url_string);
                    }
                }
                append_imported_stylesheets(
                    out,
                    imported,
                    directive.layer,
                    directive.supports.as_deref(),
                    directive.media.as_deref(),
                );
            }
            push_stylesheet_chunk(&chars[cursor..], stylesheet_url, out);
            return Ok(());
        }
    }

    out.push(resolve_stylesheet_asset_urls(css, stylesheet_url));
    Ok(())
}

fn push_stylesheet_chunk(
    chars: &[char],
    stylesheet_url: Option<&crate::http::Url>,
    out: &mut Vec<String>,
) {
    let css: String = chars.iter().collect();
    if !css.trim().is_empty() {
        out.push(resolve_stylesheet_asset_urls(css, stylesheet_url));
    }
}

pub(crate) fn append_imported_stylesheets(
    out: &mut Vec<String>,
    imported: Vec<String>,
    layer: Option<ImportLayer>,
    supports: Option<&str>,
    media: Option<&str>,
) {
    if imported.is_empty() && layer.is_none() {
        return;
    }
    if layer.is_none() && supports.is_none() && media.is_none() {
        out.extend(imported);
        return;
    }

    let mut css = imported.join("\n");
    if let Some(layer) = layer {
        css = match layer {
            ImportLayer::Anonymous => format!("@layer {{\n{css}\n}}"),
            ImportLayer::Named(name) => format!("@layer {name} {{\n{css}\n}}"),
        };
    }
    if let Some(condition) = supports {
        css = format!("@supports {condition} {{\n{css}\n}}");
    }
    if let Some(query) = media {
        css = format!("@media {query} {{\n{css}\n}}");
    }
    out.push(css);
}

/// Resolve relative `url()` references against the stylesheet URL, rather than
/// the document URL. CSS URLs are scoped to the stylesheet that contains them.
pub(crate) fn resolve_stylesheet_asset_urls(
    css: String,
    stylesheet_url: Option<&crate::http::Url>,
) -> String {
    let Some(base) = stylesheet_url else {
        return css;
    };

    let mut output = String::with_capacity(css.len());
    let mut rest = css.as_str();
    while let Some(start) = find_ascii_case_insensitive_url(rest) {
        output.push_str(&rest[..start]);
        let after_open = &rest[start + 4..];
        let Some(end) = find_url_closing_parenthesis(after_open) else {
            output.push_str(&rest[start..]);
            return output;
        };

        let raw = after_open[..end].trim();
        let unquoted = raw
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .or_else(|| {
                raw.strip_prefix('\'')
                    .and_then(|value| value.strip_suffix('\''))
            })
            .unwrap_or(raw);
        if unquoted.starts_with("data:") || unquoted.starts_with('#') {
            output.push_str(&rest[start..start + 4 + end + 1]);
        } else if let Ok(resolved) = resolve_url(base, unquoted) {
            output.push_str("url(\"");
            output.push_str(&resolved.to_string());
            output.push_str("\")");
        } else {
            output.push_str(&rest[start..start + 4 + end + 1]);
        }
        rest = &after_open[end + 1..];
    }
    output.push_str(rest);
    output
}

fn find_ascii_case_insensitive_url(input: &str) -> Option<usize> {
    input
        .as_bytes()
        .windows(4)
        .position(|candidate| candidate.eq_ignore_ascii_case(b"url("))
}

fn find_url_closing_parenthesis(input: &str) -> Option<usize> {
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in input.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        match quote {
            Some(active) if ch == active => quote = None,
            Some(_) => {}
            None if ch == '"' || ch == '\'' => quote = Some(ch),
            None if ch == ')' => return Some(index),
            None => {}
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ImportLayer {
    Anonymous,
    Named(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImportDirective {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) href: String,
    pub(crate) layer: Option<ImportLayer>,
    pub(crate) supports: Option<String>,
    pub(crate) media: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImportPrelude {
    pub(crate) href: String,
    pub(crate) layer: Option<ImportLayer>,
    pub(crate) supports: Option<String>,
    pub(crate) media: Option<String>,
}

pub(crate) fn extract_import_directives(css: &str) -> Vec<ImportDirective> {
    let mut directives = Vec::new();
    let chars: Vec<char> = css.chars().collect();
    let mut index = 0usize;
    let mut in_string = None::<char>;
    let mut paren_depth = 0usize;
    let mut curly_depth = 0usize;

    while index < chars.len() {
        let ch = chars[index];

        if let Some(quote) = in_string {
            if ch == '\\' && index + 1 < chars.len() {
                index += 2;
                continue;
            }
            if ch == quote {
                in_string = None;
            }
            index += 1;
            continue;
        }

        if ch == '/' && chars.get(index + 1) == Some(&'*') {
            index += 2;
            while index + 1 < chars.len() && !(chars[index] == '*' && chars[index + 1] == '/') {
                index += 1;
            }
            index = (index + 2).min(chars.len());
            continue;
        }

        match ch {
            '"' | '\'' => {
                in_string = Some(ch);
                index += 1;
                continue;
            }
            '(' => {
                paren_depth += 1;
                index += 1;
                continue;
            }
            ')' => {
                paren_depth = paren_depth.saturating_sub(1);
                index += 1;
                continue;
            }
            '{' => {
                curly_depth += 1;
                index += 1;
                continue;
            }
            '}' => {
                curly_depth = curly_depth.saturating_sub(1);
                index += 1;
                continue;
            }
            _ => {}
        }

        if paren_depth == 0 && curly_depth == 0 && at_import_starts_at(&chars, index) {
            let directive_start = index;
            let mut prelude_start = index + 7;
            while prelude_start < chars.len() && chars[prelude_start].is_ascii_whitespace() {
                prelude_start += 1;
            }
            let mut cursor = prelude_start;
            let mut local_in_string = None::<char>;
            let mut local_paren_depth = 0usize;
            while cursor < chars.len() {
                let c = chars[cursor];
                if let Some(quote) = local_in_string {
                    if c == '\\' && cursor + 1 < chars.len() {
                        cursor += 2;
                        continue;
                    }
                    if c == quote {
                        local_in_string = None;
                    }
                    cursor += 1;
                    continue;
                }
                if c == '"' || c == '\'' {
                    local_in_string = Some(c);
                    cursor += 1;
                    continue;
                }
                if c == '(' {
                    local_paren_depth += 1;
                    cursor += 1;
                    continue;
                }
                if c == ')' {
                    local_paren_depth = local_paren_depth.saturating_sub(1);
                    cursor += 1;
                    continue;
                }
                if c == ';' && local_paren_depth == 0 {
                    let prelude: String = chars[prelude_start..cursor].iter().collect();
                    if let Some(parsed) = parse_import_prelude(&prelude) {
                        directives.push(ImportDirective {
                            start: directive_start,
                            end: cursor + 1,
                            href: parsed.href,
                            layer: parsed.layer,
                            supports: parsed.supports,
                            media: parsed.media,
                        });
                    }
                    cursor += 1;
                    break;
                }
                cursor += 1;
            }
            index = cursor;
            continue;
        }

        index += 1;
    }

    directives
}

pub(crate) fn at_import_starts_at(chars: &[char], index: usize) -> bool {
    let target: [char; 7] = ['@', 'i', 'm', 'p', 'o', 'r', 't'];
    if index + target.len() > chars.len() {
        return false;
    }
    for (offset, expected) in target.iter().enumerate() {
        if chars[index + offset].to_ascii_lowercase() != *expected {
            return false;
        }
    }
    if index + target.len() < chars.len() {
        let next = chars[index + target.len()];
        if next.is_ascii_alphanumeric() || next == '-' || next == '_' {
            return false;
        }
    }
    true
}

pub(crate) fn parse_import_prelude(prelude: &str) -> Option<ImportPrelude> {
    let prelude = trim_import_spacing(prelude.trim())?;
    if prelude.is_empty() {
        return None;
    }

    let (href, trailing) = if prelude
        .get(0..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("url("))
    {
        let rest = &prelude[4..];
        let close = find_url_closing_parenthesis(rest)?;
        let content = rest[..close].trim();
        let href = if let Some(quoted) = unquote_css_token(content) {
            quoted
        } else {
            if content.starts_with('"') || content.starts_with('\'') {
                return None;
            }
            non_empty_token(content)?
        };
        (href, rest[close + 1..].trim())
    } else if prelude.starts_with('"') || prelude.starts_with('\'') {
        let quote = prelude.chars().next()?;
        let mut escaped = false;
        let mut parsed = None;
        for (index, ch) in prelude.char_indices().skip(1) {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == quote {
                let value = prelude[1..index].trim();
                if value.is_empty() {
                    return None;
                }
                parsed = Some((value.to_string(), prelude[index + ch.len_utf8()..].trim()));
                break;
            }
        }
        parsed?
    } else {
        return None;
    };

    let mut remaining = trim_import_spacing(trailing)?;
    let mut layer = None;
    if let Some(after_keyword) = strip_ascii_keyword(remaining, "layer") {
        if after_keyword.starts_with('(') {
            let close = find_matching_css_parenthesis(after_keyword)?;
            if let Some(name) = normalize_import_layer_name(&after_keyword[1..close]) {
                layer = Some(ImportLayer::Named(name));
                remaining = trim_import_spacing(&after_keyword[close + 1..])?;
            }
        } else if after_keyword.is_empty()
            || after_keyword
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
            || after_keyword.starts_with("/*")
        {
            layer = Some(ImportLayer::Anonymous);
            remaining = trim_import_spacing(after_keyword)?;
        }
    }

    let mut supports = None;
    if remaining
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("supports"))
        && remaining[8..].starts_with('(')
    {
        let function = &remaining[8..];
        let close = find_matching_css_parenthesis(function)?;
        let condition = function[1..close].trim();
        crate::css::supports_condition_result(condition)?;
        supports = Some(condition.to_string());
        remaining = trim_import_spacing(&function[close + 1..])?;
    }

    let media = if remaining.is_empty() {
        None
    } else {
        crate::css::parse_media_query_list(remaining)?;
        Some(remaining.to_string())
    };

    Some(ImportPrelude {
        href,
        layer,
        supports,
        media,
    })
}

pub(crate) fn parse_import_rule(input: &str) -> Option<ImportPrelude> {
    let input = trim_import_spacing(input)?.trim_end();
    let keyword = input.get(..7)?;
    if !keyword.eq_ignore_ascii_case("@import") {
        return None;
    }
    let prelude = input[7..].trim().strip_suffix(';')?.trim_end();
    parse_import_prelude(prelude)
}

fn trim_import_spacing(mut input: &str) -> Option<&str> {
    loop {
        input = input.trim_start_matches(char::is_whitespace);
        let Some(comment) = input.strip_prefix("/*") else {
            return Some(input);
        };
        let close = comment.find("*/")?;
        input = &comment[close + 2..];
    }
}

fn strip_ascii_keyword<'a>(input: &'a str, keyword: &str) -> Option<&'a str> {
    let prefix = input.get(..keyword.len())?;
    prefix
        .eq_ignore_ascii_case(keyword)
        .then(|| &input[keyword.len()..])
}

fn find_matching_css_parenthesis(input: &str) -> Option<usize> {
    if !input.starts_with('(') {
        return None;
    }
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    let mut comment = false;
    let mut chars = input.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if comment {
            if ch == '*' && chars.peek().is_some_and(|(_, next)| *next == '/') {
                chars.next();
                comment = false;
            }
            continue;
        }
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote.is_some() {
            escaped = true;
            continue;
        }
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            }
            continue;
        }
        if ch == '/' && chars.peek().is_some_and(|(_, next)| *next == '*') {
            chars.next();
            comment = true;
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '(' => depth += 1,
            ')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn normalize_import_layer_name(input: &str) -> Option<String> {
    let names = crate::css::parse_layer_name_list(input)?;
    match names.as_slice() {
        [name] => Some(name.join(".")),
        _ => None,
    }
}

pub(crate) fn unquote_css_token(token: &str) -> Option<String> {
    let token = token.trim();
    let first = token.chars().next()?;
    if first != '"' && first != '\'' {
        return None;
    }
    let mut escaped = false;
    for (index, ch) in token.char_indices().skip(1) {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == first {
            let value = token[1..index].trim();
            if value.is_empty() {
                return None;
            }
            if !token[index + ch.len_utf8()..].trim().is_empty() {
                return None;
            }
            return Some(value.to_string());
        }
    }
    None
}

pub(crate) fn non_empty_token(token: &str) -> Option<String> {
    let token = token.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

pub(crate) fn fetch_relative_stylesheet(
    base: &crate::http::Url,
    href: &str,
    client: &mut Option<crate::http::Client>,
    document_base: Option<&crate::http::Url>,
) -> Option<(String, crate::http::Url)> {
    let resolved = resolve_relative_stylesheet_url(base, href, document_base)?;
    let css = fetch_stylesheet_by_url(&resolved, client, document_base)?;
    Some((css, resolved))
}

pub(crate) fn resolve_relative_stylesheet_url(
    base: &crate::http::Url,
    href: &str,
    document_base: Option<&crate::http::Url>,
) -> Option<crate::http::Url> {
    let resolved = resolve_url(base, href).ok()?;
    // Cross-origin author stylesheets are normal on the web, but allowing
    // arbitrary destinations would let a public page probe private services.
    // Keep same-origin behavior (including local test servers), and require
    // cross-origin stylesheets to resolve exclusively to public HTTPS addresses.
    if !same_origin(&resolved, document_base.unwrap_or(base))
        && !is_public_cross_origin_stylesheet_url(&resolved)
    {
        return None;
    }
    Some(resolved)
}

fn is_public_cross_origin_stylesheet_url(url: &crate::http::Url) -> bool {
    if url.scheme() != "https" {
        return false;
    }
    match url.host().parse() {
        Ok(ip) => crate::http::is_public_ip(ip),
        Err(_) => true,
    }
}

pub(crate) fn fetch_stylesheet_by_url(
    resolved: &crate::http::Url,
    client: &mut Option<crate::http::Client>,
    document_base: Option<&crate::http::Url>,
) -> Option<String> {
    let url_str = resolved.to_string();
    let c = client.as_mut()?;
    let resp = if document_base.is_some_and(|base| !same_origin(resolved, base)) {
        c.get_public(&url_str).ok()?
    } else {
        c.get(&url_str).ok()?
    };
    if resp.status_code() != 200 {
        return None;
    }
    let body = resp.body();
    if body.len() > MAX_EXTERNAL_STYLESHEET_BYTES {
        return None;
    }
    std::str::from_utf8(body).ok().map(|s| s.to_owned())
}

pub(crate) fn parse_stylesheet_forgiving(input: &str) -> Stylesheet {
    if let Ok(stylesheet) = parse_stylesheet(input) {
        return stylesheet;
    }

    let mut rules = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut prev_backslash = false;

    for ch in input.chars() {
        current.push(ch);
        if prev_backslash {
            prev_backslash = false;
            continue;
        }
        if ch == '\\' {
            prev_backslash = true;
            continue;
        }
        match ch {
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let trimmed = current.trim_start_matches(|c: char| c.is_ascii_whitespace());
                    if !trimmed.is_empty() {
                        if let Ok(stylesheet) = parse_stylesheet(trimmed) {
                            rules.extend(stylesheet.rules);
                        } else if let Some(rule) = salvage_nested_at_rule(trimmed) {
                            rules.push(rule);
                        } else if let Some(rule) = salvage_style_rule(trimmed) {
                            rules.push(crate::css::Rule::Style(rule));
                        }
                    }
                    current.clear();
                }
            }
            _ => {}
        }
    }

    Stylesheet { rules }
}

fn salvage_nested_at_rule(input: &str) -> Option<crate::css::Rule> {
    let open = input.find('{')?;
    let close = input.rfind('}')?;
    if close <= open {
        return None;
    }

    let header = input[..open].trim();
    let after_at = header.strip_prefix('@')?;
    let name_end = after_at
        .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-'))
        .unwrap_or(after_at.len());
    if name_end == 0 {
        return None;
    }
    let (name, prelude) = after_at.split_at(name_end);

    let nested = parse_stylesheet_forgiving(&input[open + 1..close]);
    if nested.rules.is_empty() {
        return None;
    }

    Some(crate::css::Rule::At(crate::css::AtRule {
        name: name.to_ascii_lowercase(),
        prelude: prelude.trim().to_string(),
        block: Some(nested.rules),
        declarations: Vec::new(),
        anonymous_layer_id: None,
    }))
}

pub(crate) fn salvage_style_rule(input: &str) -> Option<crate::css::StyleRule> {
    let open = input.find('{')?;
    let close = input.rfind('}')?;
    if close <= open {
        return None;
    }

    let selector = input[..open].trim();
    let body = &input[open + 1..close];
    let mut selectors = None;
    let mut declarations = Vec::new();

    for declaration in split_declarations_forgiving(body) {
        let normalized = normalize_unquoted_urls(&declaration);
        let candidate = format!("{selector} {{ {normalized}; }}");
        let Ok(stylesheet) = parse_stylesheet(&candidate) else {
            continue;
        };
        let Some(crate::css::Rule::Style(rule)) = stylesheet.rules.into_iter().next() else {
            continue;
        };
        if selectors.is_none() {
            selectors = Some(rule.selectors);
        }
        declarations.extend(rule.declarations);
    }

    if declarations.is_empty() {
        return None;
    }

    Some(crate::css::StyleRule {
        selectors: selectors?,
        declarations,
    })
}

pub(crate) fn normalize_unquoted_urls(input: &str) -> String {
    let mut output = String::new();
    let mut index = 0usize;

    while let Some(relative_start) = input[index..].find("url(") {
        let start = index + relative_start;
        output.push_str(&input[index..start + 4]);
        let content_start = start + 4;
        let Some(relative_end) = input[content_start..].find(')') else {
            output.push_str(&input[content_start..]);
            return output;
        };
        let end = content_start + relative_end;
        let content = input[content_start..end].trim();
        if content.starts_with('"') || content.starts_with('\'') {
            output.push_str(content);
        } else {
            output.push('"');
            output.push_str(content);
            output.push('"');
        }
        output.push(')');
        index = end + 1;
    }

    output.push_str(&input[index..]);
    output
}

pub(crate) fn split_declarations_forgiving(input: &str) -> Vec<String> {
    let mut declarations = Vec::new();
    let mut current = String::new();
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut in_string = None::<char>;
    let chars: Vec<char> = input.chars().collect();
    let mut index = 0usize;

    while index < chars.len() {
        let ch = chars[index];
        if let Some(quote) = in_string {
            current.push(ch);
            if ch == quote {
                in_string = None;
            } else if ch == '\\' && index + 1 < chars.len() {
                index += 1;
                current.push(chars[index]);
            }
            index += 1;
            continue;
        }

        if ch == '/' && chars.get(index + 1) == Some(&'*') {
            index += 2;
            while index + 1 < chars.len() && !(chars[index] == '*' && chars[index + 1] == '/') {
                index += 1;
            }
            index = (index + 2).min(chars.len());
            continue;
        }

        match ch {
            '"' | '\'' => {
                in_string = Some(ch);
                current.push(ch);
            }
            '(' => {
                paren_depth += 1;
                current.push(ch);
            }
            ')' => {
                paren_depth = paren_depth.saturating_sub(1);
                current.push(ch);
            }
            '[' => {
                bracket_depth += 1;
                current.push(ch);
            }
            ']' => {
                bracket_depth = bracket_depth.saturating_sub(1);
                current.push(ch);
            }
            ';' if paren_depth == 0 && bracket_depth == 0 => {
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    declarations.push(trimmed.to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
        index += 1;
    }

    let trimmed = current.trim();
    if !trimmed.is_empty() {
        declarations.push(trimmed.to_string());
    }

    declarations
}

/// Returns true if the media attribute value applies to screen rendering.
///
/// Matches:
/// - Empty string or missing attribute (defaults to "all")
/// - "all" or "screen" as whole-word media types
/// - Comma-separated lists (e.g., "print, screen")
/// - Media queries with "only" modifier (e.g., "only screen")
///
/// Does NOT match:
/// - "print" or other non-screen media types
/// - "not screen" (negated screen)
/// - Substrings (e.g., "small" does NOT match just because it contains "all")
pub(crate) fn matches_screen_media(media: Option<&str>) -> bool {
    let media = match media {
        None => return true, // No media attr = all media
        Some(s) => s.trim(),
    };

    if media.is_empty() {
        return true; // Empty media attr = all media
    }

    // Parse as comma-separated list of media queries
    for query in media.split(',') {
        let query = query.trim();
        if query.is_empty() {
            // Empty entry means "all"
            return true;
        }

        let query_lower = query.to_ascii_lowercase();
        let mut tokens = query_lower.split_whitespace();

        let first = tokens.next();
        let (modifier, media_type) = match first {
            None => {
                // Empty query -> defaults to "all"
                (None::<&str>, Some("all"))
            }
            Some(tok) if tok == "not" || tok == "only" => {
                // Modifier followed by media type
                let mt = tokens.next();
                (Some(tok), mt)
            }
            Some(tok) if tok.starts_with('(') => {
                // Leading feature without explicit type (e.g., "(min-width: 800px)")
                // defaults to "all"
                (None::<&str>, Some("all"))
            }
            Some(tok) => {
                // First token is the media type
                (None::<&str>, Some(tok))
            }
        };

        let media_type = media_type.unwrap_or("all");
        let is_screen_like = media_type == "screen" || media_type == "all";

        if !is_screen_like {
            // Non-screen media type such as "print", "speech", etc.
            continue;
        }

        match modifier {
            Some("not") => {
                // "not screen" or "not all" explicitly excludes screen
                continue;
            }
            _ => {
                // Matches screen/all (with or without "only")
                return true;
            }
        }
    }

    // No query matched screen/all
    false
}

/// Checks if two URLs have the same origin (scheme + host + port).
pub(crate) fn same_origin(a: &crate::http::Url, b: &crate::http::Url) -> bool {
    a.scheme() == b.scheme() && a.host() == b.host() && a.port() == b.port()
}

/// Recursively finds all `<base>` elements in document order.
pub(crate) fn find_base_elements(node: &NodeHandle, result: &mut Vec<NodeHandle>) {
    if node.node_type() == crate::dom::NodeType::Element
        && node.tag_name().as_deref() == Some("base")
    {
        result.push(node.clone());
    }
    for child in node.child_nodes() {
        find_base_elements(&child, result);
    }
}

/// Extracts the document's base URL from the first `<base href="...">` element with a valid href.
///
/// Scans all `<base>` elements in document order and uses the first one with a
/// non-empty, resolvable `href`. For SSRF protection, absolute URLs are only honored
/// if they have the same origin (scheme + host + port) as the fallback_base.
/// Returns the fallback base if no valid same-origin `<base>` is found.
pub(crate) fn extract_document_base_url(
    document: &NodeHandle,
    fallback_base: Option<&crate::http::Url>,
) -> Option<crate::http::Url> {
    let mut base_elements = Vec::new();
    find_base_elements(document, &mut base_elements);

    for base_elem in base_elements {
        if let Some(attrs) = base_elem.attributes()
            && let Some(href) = attrs.get("href")
        {
            let href = href.trim();
            if href.is_empty() {
                continue; // Skip empty href, try next <base>
            }

            // Absolute URL
            if href.contains("://") {
                if let Ok(url) = href.parse::<crate::http::Url>() {
                    // SSRF protection: only honor same-origin absolute base URLs
                    if let Some(original) = fallback_base
                        && same_origin(&url, original)
                    {
                        return Some(url);
                    }
                    // If no fallback_base provided, don't enable fetching via <base>
                    continue;
                }
                continue; // Invalid absolute URL, try next <base>
            }

            // Relative URL (resolve against fallback_base)
            if let Some(base) = fallback_base
                && let Ok(url) = resolve_url(base, href)
            {
                // Relative URLs always resolve to same origin
                return Some(url);
            }
        }
    }
    fallback_base.cloned()
}

pub(crate) fn collect_text_contents(node: &NodeHandle) -> String {
    let mut text = String::new();
    for child in node.child_nodes() {
        match child.node_type() {
            NodeType::Text => {
                if let Some(data) = child.data() {
                    text.push_str(&data);
                }
            }
            NodeType::Element => text.push_str(&collect_text_contents(&child)),
            _ => {}
        }
    }
    text
}

pub(crate) fn materialize_local_assets(
    node: &NodeHandle,
    base_path: &std::path::Path,
) -> Result<(), PaintError> {
    if node.node_type() == NodeType::Element {
        match node.tag_name().as_deref() {
            Some("img") => rewrite_local_asset_attribute(node, "src", base_path)?,
            Some("link") => rewrite_local_asset_attribute(node, "href", base_path)?,
            _ => {}
        }
    }

    for child in node.child_nodes() {
        materialize_local_assets(&child, base_path)?;
    }

    Ok(())
}

pub(crate) fn rewrite_local_asset_attribute(
    node: &NodeHandle,
    attribute_name: &str,
    base_path: &std::path::Path,
) -> Result<(), PaintError> {
    let attributes = node.attributes().unwrap_or_default();
    let Some(value) = attributes.get(attribute_name) else {
        return Ok(());
    };
    if value.is_empty()
        || value.starts_with("data:")
        || value.starts_with("http://")
        || value.starts_with("https://")
        || value.starts_with('#')
        || value.contains(':')
    {
        return Ok(());
    }

    let asset_path = base_path.join(value);
    if !asset_path.is_file() {
        return Ok(());
    }

    let mime_type = match asset_path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("png") => "image/png",
        Some(ext) if ext.eq_ignore_ascii_case("css") => "text/css",
        _ => return Ok(()),
    };

    let data = std::fs::read(asset_path).map_err(|_| PaintError::InvalidDataUri)?;
    let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, data);
    node.set_attribute(attribute_name, format!("data:{mime_type};base64,{encoded}"));

    Ok(())
}

/// A loaded web font together with its `@font-face` variant descriptors.
pub(crate) struct WebFont {
    pub family: String,
    pub weight: crate::font::FontWeight,
    pub style: crate::font::FontStyle,
    pub font: Arc<Font>,
}

/// Extract `@font-face` rules from parsed stylesheets, fetch the font files
/// via HTTP, and return loaded `WebFont` objects with variant information.
///
/// Unlike the previous implementation, the same family can appear multiple
/// times with different weight/style variants (e.g. regular + bold + italic).
/// Fonts that fail to fetch or parse are silently skipped (fallback to system fonts).
pub(crate) fn fetch_font_face_fonts(
    stylesheets: &[Stylesheet],
    base_url: Option<&crate::http::Url>,
) -> Vec<WebFont> {
    let mut web_fonts = Vec::new();
    let mut client: Option<crate::http::Client> = None;
    // Deduplicate by (family, weight, style) tuple so we don't re-fetch the same variant
    let mut seen_variants: HashSet<(String, u16, u8)> = HashSet::new();

    for sheet in stylesheets {
        for ff_rule in extract_font_face_rules(sheet) {
            let family_lower = ff_rule.font_family.to_lowercase();

            // Skip WOFF2 when format hint says so (not supported yet)
            if let Some(ref fmt) = ff_rule.format
                && fmt.eq_ignore_ascii_case("woff2")
            {
                continue;
            }

            let url_str = &ff_rule.src_url;

            // Parse variant descriptors
            let weight =
                crate::font::FontWeight::parse(ff_rule.font_weight.as_deref().unwrap_or("normal"));
            let style =
                crate::font::FontStyle::parse(ff_rule.font_style.as_deref().unwrap_or("normal"));

            // Deduplicate: encode style as u8 (0=normal, 1=italic, 2=oblique)
            let style_ord: u8 = match style {
                crate::font::FontStyle::Normal => 0,
                crate::font::FontStyle::Italic => 1,
                crate::font::FontStyle::Oblique => 2,
            };
            let variant_key = (family_lower.clone(), weight.0, style_ord);
            if seen_variants.contains(&variant_key) {
                continue;
            }

            // Embedded fonts do not need a network URL or a document base.
            // Use the shared data-URL decoder and the same decoded size limit.
            let data = if url_str
                .get(..5)
                .is_some_and(|s| s.eq_ignore_ascii_case("data:"))
            {
                match crate::http::parse_data_uri(url_str) {
                    Some(uri) if uri.data.len() <= MAX_FONT_BYTES => uri.data,
                    _ => continue,
                }
            } else {
                // Keep the existing destination policy for network fonts.
                let Some(base) = base_url else { continue };
                let resolved = match resolve_url(base, url_str) {
                    Ok(url)
                        if same_origin(&url, base)
                            || is_public_cross_origin_stylesheet_url(&url) =>
                    {
                        url
                    }
                    _ => continue,
                };
                let cross_origin = !same_origin(&resolved, base);
                match fetch_font_bytes(&resolved.to_string(), &mut client, cross_origin) {
                    Some(data) => data,
                    None => continue,
                }
            };

            // Load font — insert into seen_variants only on success to allow
            // retrying with a different src URL if this one fails to parse.
            match Font::load_from_bytes(data) {
                Ok(font) => {
                    seen_variants.insert(variant_key);
                    web_fonts.push(WebFont {
                        family: ff_rule.font_family.clone(),
                        weight,
                        style,
                        font: Arc::new(font),
                    });
                }
                Err(_) => continue,
            }
        }
    }

    web_fonts
}

/// Maximum allowed font file size in bytes (10 MB).
const MAX_FONT_BYTES: usize = 10_000_000;

/// Fetch raw bytes from a URL (HTTP/HTTPS).
fn fetch_font_bytes(
    url: &str,
    client: &mut Option<crate::http::Client>,
    public_only: bool,
) -> Option<Vec<u8>> {
    if client.is_none() {
        *client = Some(crate::http::Client::new());
    }
    let c = client.as_mut()?;

    let response = if public_only {
        c.get_public(url).ok()?
    } else {
        c.get(url).ok()?
    };

    if response.status_code() < 200 || response.status_code() >= 300 {
        return None;
    }

    let body = response.body();
    if body.len() > MAX_FONT_BYTES {
        return None;
    }

    Some(body.to_vec())
}

#[cfg(test)]
mod forgiving_tests {
    use super::parse_stylesheet_forgiving;

    #[test]
    fn preserves_valid_rules_inside_invalid_media_block() {
        let css = "@media screen and (min-width:48em){.before{color:red}[data-broken=]{color:black}.after{display:none}}";
        let stylesheet = parse_stylesheet_forgiving(css);

        let crate::css::Rule::At(media) = &stylesheet.rules[0] else {
            panic!("expected a recovered media rule");
        };
        assert_eq!(media.name, "media");
        assert_eq!(media.prelude, "screen and (min-width:48em)");
        assert_eq!(media.block.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn preserves_other_nested_at_rule_names() {
        let css = "@supports (display:grid){.valid{display:block}[data-broken=]{color:black}}";
        let stylesheet = parse_stylesheet_forgiving(css);

        let crate::css::Rule::At(supports) = &stylesheet.rules[0] else {
            panic!("expected a recovered supports rule");
        };
        assert_eq!(supports.name, "supports");
        assert_eq!(supports.prelude, "(display:grid)");
        assert_eq!(supports.block.as_ref().unwrap().len(), 1);
    }
}
