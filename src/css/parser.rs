//! CSS parser: selectors, declarations, and value parsing.

use super::{CssParseError, CssToken, Selector, SelectorPart, SimpleSelector, AttributeOperator, Combinator, Declaration, Value, Rule, StyleRule, AtRule, FontFaceRule, Stylesheet};
use super::tokenizer::{tokenize, render_tokens};
use super::shorthand::expand_shorthand;

/// Extract all `@font-face` rules from a stylesheet.
///
/// Returns a list of `FontFaceRule` values with the font-family, src URL,
/// optional format hint, font-weight, and font-style descriptors.
pub fn extract_font_face_rules(stylesheet: &Stylesheet) -> Vec<FontFaceRule> {
    let mut rules = Vec::new();
    collect_font_face_rules_recursive(&stylesheet.rules, &mut rules);
    rules
}

fn collect_font_face_rules_recursive(rules: &[Rule], out: &mut Vec<FontFaceRule>) {
    for rule in rules {
        match rule {
            Rule::FontFace(ff) => out.push(ff.clone()),
            Rule::At(at) => {
                if let Some(block) = &at.block {
                    collect_font_face_rules_recursive(block, out);
                }
            }
            _ => {}
        }
    }
}

/// Parses a stylesheet from CSS source.
pub fn parse_stylesheet(input: &str) -> Result<Stylesheet, CssParseError> {
    let tokens = tokenize(input)?;
    Parser::new(tokens).parse_stylesheet()
}

struct Parser {
    tokens: Vec<CssToken>,
    index: usize,
}

impl Parser {
    fn new(tokens: Vec<CssToken>) -> Self {
        Self { tokens, index: 0 }
    }

    fn parse_stylesheet(&mut self) -> Result<Stylesheet, CssParseError> {
        let mut rules = Vec::new();
        while self.peek().is_some() {
            self.skip_whitespace();
            if self.peek().is_none() {
                break;
            }
            rules.push(self.parse_rule()?);
            self.skip_whitespace();
        }
        Ok(Stylesheet { rules })
    }

    fn parse_rule(&mut self) -> Result<Rule, CssParseError> {
        match self.peek() {
            Some(CssToken::AtKeyword(_)) => self.parse_at_rule(),
            _ => self.parse_style_rule(),
        }
    }

    fn parse_at_rule(&mut self) -> Result<Rule, CssParseError> {
        let name = match self.next() {
            Some(CssToken::AtKeyword(name)) => name,
            _ => return Err(CssParseError::ExpectedToken("@keyword")),
        };

        let mut prelude_tokens = Vec::new();
        while let Some(token) = self.peek() {
            match token {
                CssToken::Semicolon => {
                    self.next();
                    return Ok(Rule::At(AtRule {
                        name,
                        prelude: render_tokens(&prelude_tokens).trim().to_string(),
                        block: None,
                        declarations: Vec::new(),
                    }));
                }
                CssToken::CurlyOpen => {
                    self.next();
                    if name.eq_ignore_ascii_case("import") {
                        return Err(CssParseError::InvalidDeclaration);
                    }

                    if name.eq_ignore_ascii_case("media") {
                        let block = self.parse_rule_block()?;
                        return Ok(Rule::At(AtRule {
                            name,
                            prelude: render_tokens(&prelude_tokens).trim().to_string(),
                            block: Some(block),
                            declarations: Vec::new(),
                        }));
                    }

                    let declarations = self.parse_declaration_list()?;

                    // Produce a structured FontFace rule when possible.
                    if name.eq_ignore_ascii_case("font-face") {
                        if let Some(ff) = build_font_face_rule(&declarations) {
                            return Ok(Rule::FontFace(ff));
                        }
                    }

                    return Ok(Rule::At(AtRule {
                        name,
                        prelude: render_tokens(&prelude_tokens).trim().to_string(),
                        block: None,
                        declarations,
                    }));
                }
                _ => prelude_tokens.push(self.next().expect("peeked token should exist")),
            }
        }

        Ok(Rule::At(AtRule {
            name,
            prelude: render_tokens(&prelude_tokens).trim().to_string(),
            block: None,
            declarations: Vec::new(),
        }))
    }

    fn parse_rule_block(&mut self) -> Result<Vec<Rule>, CssParseError> {
        let mut rules = Vec::new();
        loop {
            self.skip_whitespace();
            match self.peek() {
                Some(CssToken::CurlyClose) => {
                    self.next();
                    break;
                }
                None => return Err(CssParseError::UnexpectedEndOfInput),
                _ => rules.push(self.parse_rule()?),
            }
        }
        Ok(rules)
    }

    fn parse_style_rule(&mut self) -> Result<Rule, CssParseError> {
        let selectors = self.parse_selector_list()?;
        self.expect_curly_open()?;
        let declarations = self.parse_declaration_list()?;
        Ok(Rule::Style(StyleRule {
            selectors,
            declarations,
        }))
    }

    fn parse_selector_list(&mut self) -> Result<Vec<Selector>, CssParseError> {
        let mut selectors = Vec::new();
        loop {
            selectors.push(self.parse_selector()?);
            self.skip_whitespace();
            match self.peek() {
                Some(CssToken::Comma) => {
                    self.next();
                    self.skip_whitespace();
                }
                Some(CssToken::CurlyOpen) => break,
                _ => break,
            }
        }
        Ok(selectors)
    }

    fn parse_selector(&mut self) -> Result<Selector, CssParseError> {
        let mut parts = Vec::new();
        let mut combinator = None;

        loop {
            let saw_whitespace = self.consume_whitespace();
            if saw_whitespace
                && !parts.is_empty()
                && combinator.is_none()
                && !matches!(
                    self.peek(),
                    Some(
                        CssToken::Delim('>')
                            | CssToken::Delim('+')
                            | CssToken::Delim('~')
                            | CssToken::CurlyOpen
                            | CssToken::Comma
                    ) | None
                )
            {
                combinator = Some(Combinator::Descendant);
            }

            match self.peek() {
                Some(CssToken::CurlyOpen) | Some(CssToken::Comma) | None => break,
                Some(CssToken::Delim('>')) => {
                    self.next();
                    combinator = Some(Combinator::Child);
                    continue;
                }
                Some(CssToken::Delim('+')) => {
                    self.next();
                    combinator = Some(Combinator::AdjacentSibling);
                    continue;
                }
                Some(CssToken::Delim('~')) => {
                    self.next();
                    combinator = Some(Combinator::GeneralSibling);
                    continue;
                }
                _ => {}
            }

            let simples = self.parse_simple_selectors()?;
            parts.push(SelectorPart {
                combinator,
                simples,
            });
            combinator = None;
        }

        if parts.is_empty() {
            return Err(CssParseError::InvalidSelector);
        }

        Ok(Selector { parts })
    }

    fn parse_simple_selectors(&mut self) -> Result<Vec<SimpleSelector>, CssParseError> {
        let mut simples = Vec::new();

        loop {
            match self.peek() {
                Some(CssToken::Ident(name)) => {
                    let name = name.clone();
                    self.next();
                    simples.push(SimpleSelector::Type(name));
                }
                Some(CssToken::Delim('*')) => {
                    self.next();
                    simples.push(SimpleSelector::Universal);
                }
                Some(CssToken::Delim('.')) => {
                    self.next();
                    let class_name = self.expect_ident()?;
                    simples.push(SimpleSelector::Class(class_name));
                }
                Some(CssToken::Hash(id)) => {
                    let id = id.clone();
                    self.next();
                    simples.push(SimpleSelector::Id(id));
                }
                Some(CssToken::BracketOpen) => {
                    self.next();
                    self.skip_whitespace();
                    let name = self.expect_ident()?;
                    self.skip_whitespace();
                    let operator = match self.peek() {
                        Some(CssToken::Delim('=')) => {
                            self.next();
                            Some(AttributeOperator::Equals)
                        }
                        Some(CssToken::Delim('~')) => {
                            self.next();
                            self.expect_delim('=')?;
                            Some(AttributeOperator::Includes)
                        }
                        Some(CssToken::Delim('^')) => {
                            self.next();
                            self.expect_delim('=')?;
                            Some(AttributeOperator::StartsWith)
                        }
                        Some(CssToken::Delim('$')) => {
                            self.next();
                            self.expect_delim('=')?;
                            Some(AttributeOperator::EndsWith)
                        }
                        Some(CssToken::Delim('*')) => {
                            self.next();
                            self.expect_delim('=')?;
                            Some(AttributeOperator::Contains)
                        }
                        Some(CssToken::Delim('|')) => {
                            self.next();
                            self.expect_delim('=')?;
                            Some(AttributeOperator::DashMatch)
                        }
                        _ => None,
                    };
                    self.skip_whitespace();
                    let value = if operator.is_some() {
                        Some(self.expect_ident_or_string()?)
                    } else {
                        None
                    };
                    self.skip_whitespace();
                    self.expect_bracket_close()?;
                    simples.push(SimpleSelector::Attribute {
                        name,
                        operator,
                        value,
                    });
                }
                Some(CssToken::Colon) => {
                    self.next();
                    let pseudo = if matches!(self.peek(), Some(CssToken::Colon)) {
                        self.next();
                        SimpleSelector::PseudoElement(self.expect_ident()?)
                    } else {
                        let name = self.expect_ident()?;
                        if matches!(self.peek(), Some(CssToken::ParenOpen)) {
                            self.next();
                            let mut argument_tokens = Vec::new();
                            let mut depth = 0usize;
                            loop {
                                match self.peek() {
                                    Some(CssToken::ParenOpen) => {
                                        depth += 1;
                                        argument_tokens.push(
                                            self.next().expect("peeked token should exist"),
                                        );
                                    }
                                    Some(CssToken::ParenClose) if depth > 0 => {
                                        depth -= 1;
                                        argument_tokens.push(
                                            self.next().expect("peeked token should exist"),
                                        );
                                    }
                                    Some(CssToken::ParenClose) | None => break,
                                    _ => {
                                        argument_tokens.push(
                                            self.next().expect("peeked token should exist"),
                                        );
                                    }
                                }
                            }
                            match self.next() {
                                Some(CssToken::ParenClose) => {}
                                _ => return Err(CssParseError::ExpectedToken(")")),
                            }
                            if name == "not" {
                                // Parse the argument as a list of simple selectors
                                let argument_str =
                                    render_tokens(&argument_tokens).trim().to_string();
                                let inner = parse_not_argument(&argument_str)?;
                                SimpleSelector::Not(inner)
                            } else {
                                let argument =
                                    render_tokens(&argument_tokens).trim().to_string();
                                SimpleSelector::PseudoClass(format!("{name}({argument})"))
                            }
                        } else {
                            SimpleSelector::PseudoClass(name)
                        }
                    };
                    simples.push(pseudo);
                }
                _ => break,
            }
        }

        if simples.is_empty() {
            return Err(CssParseError::InvalidSelector);
        }

        Ok(simples)
    }

    fn parse_declaration_list(&mut self) -> Result<Vec<Declaration>, CssParseError> {
        let mut declarations = Vec::new();
        loop {
            self.skip_whitespace();
            match self.peek() {
                Some(CssToken::CurlyClose) => {
                    self.next();
                    break;
                }
                None => return Err(CssParseError::UnexpectedEndOfInput),
                _ => {
                    let mut parsed = self.parse_declaration()?;
                    declarations.append(&mut parsed);
                    self.skip_whitespace();
                    if matches!(self.peek(), Some(CssToken::Semicolon)) {
                        self.next();
                    }
                }
            }
        }
        Ok(declarations)
    }

    fn parse_declaration(&mut self) -> Result<Vec<Declaration>, CssParseError> {
        let name = self.expect_ident()?.to_ascii_lowercase();
        self.skip_whitespace();
        self.expect_colon()?;
        self.skip_whitespace();

        let mut value_tokens = Vec::new();
        while let Some(token) = self.peek() {
            match token {
                CssToken::Semicolon | CssToken::CurlyClose => break,
                _ => value_tokens.push(self.next().expect("peeked token should exist")),
            }
        }

        let (value_tokens, important) = split_important(&value_tokens);
        let value = parse_value_tokens(&value_tokens)?;
        Ok(expand_shorthand(&name, value, important))
    }

    fn consume_whitespace(&mut self) -> bool {
        let mut consumed = false;
        while matches!(self.peek(), Some(CssToken::Whitespace)) {
            consumed = true;
            self.next();
        }
        consumed
    }

    fn skip_whitespace(&mut self) {
        let _ = self.consume_whitespace();
    }

    fn expect_ident(&mut self) -> Result<String, CssParseError> {
        match self.next() {
            Some(CssToken::Ident(name)) => Ok(name),
            _ => Err(CssParseError::ExpectedToken("identifier")),
        }
    }

    fn expect_ident_or_string(&mut self) -> Result<String, CssParseError> {
        match self.next() {
            Some(CssToken::Ident(value)) | Some(CssToken::String(value)) => Ok(value),
            Some(CssToken::Hash(value)) => Ok(value),
            _ => Err(CssParseError::ExpectedToken("identifier or string")),
        }
    }

    fn expect_colon(&mut self) -> Result<(), CssParseError> {
        match self.next() {
            Some(CssToken::Colon) => Ok(()),
            _ => Err(CssParseError::ExpectedToken(":")),
        }
    }

    fn expect_curly_open(&mut self) -> Result<(), CssParseError> {
        match self.next() {
            Some(CssToken::CurlyOpen) => Ok(()),
            _ => Err(CssParseError::ExpectedToken("{")),
        }
    }

    fn expect_bracket_close(&mut self) -> Result<(), CssParseError> {
        match self.next() {
            Some(CssToken::BracketClose) => Ok(()),
            _ => Err(CssParseError::ExpectedToken("]")),
        }
    }

    fn expect_delim(&mut self, expected: char) -> Result<(), CssParseError> {
        match self.next() {
            Some(CssToken::Delim(found)) if found == expected => Ok(()),
            _ => Err(CssParseError::ExpectedToken("delimiter")),
        }
    }

    fn peek(&self) -> Option<&CssToken> {
        self.tokens.get(self.index)
    }

    fn next(&mut self) -> Option<CssToken> {
        let token = self.tokens.get(self.index).cloned()?;
        self.index += 1;
        Some(token)
    }
}

fn parse_value_tokens(tokens: &[CssToken]) -> Result<Value, CssParseError> {
    parse_value_tokens_with_mode(tokens, false)
}

fn parse_value_tokens_with_mode(
    tokens: &[CssToken],
    preserve_math_delims: bool,
) -> Result<Value, CssParseError> {
    let tokens: Vec<CssToken> = tokens.to_vec();

    if tokens.is_empty() {
        return Err(CssParseError::InvalidDeclaration);
    }

    let non_whitespace: Vec<CssToken> = tokens
        .iter()
        .filter(|token| !matches!(token, CssToken::Whitespace))
        .cloned()
        .collect();

    if non_whitespace.len() == 1 {
        return parse_single_value(&non_whitespace[0]);
    }

    if non_whitespace.is_empty() {
        return Err(CssParseError::InvalidDeclaration);
    }

    let values = parse_value_sequence_with_mode(&tokens, preserve_math_delims)?;
    if values.len() == 1 {
        Ok(values.into_iter().next().expect("single item must exist"))
    } else {
        Ok(Value::List(values))
    }
}

fn parse_function_arguments_with_mode(
    tokens: &[CssToken],
    preserve_math_delims: bool,
) -> Result<Vec<Value>, CssParseError> {
    let mut arguments = Vec::new();
    let mut depth = 0usize;
    let mut segment = Vec::new();
    for token in tokens {
        match token {
            CssToken::ParenOpen => depth += 1,
            CssToken::ParenClose => depth = depth.saturating_sub(1),
            CssToken::Comma if depth == 0 => {
                if !segment.is_empty() {
                    arguments.push(parse_value_tokens_with_mode(
                        &segment,
                        preserve_math_delims,
                    )?);
                    segment.clear();
                }
                continue;
            }
            _ => {}
        }
        segment.push(token.clone());
    }
    if !segment.is_empty() {
        arguments.push(parse_value_tokens_with_mode(
            &segment,
            preserve_math_delims,
        )?);
    }
    Ok(arguments)
}

fn parse_value_sequence_with_mode(
    tokens: &[CssToken],
    preserve_math_delims: bool,
) -> Result<Vec<Value>, CssParseError> {
    let mut values = Vec::new();
    let mut index = 0;

    while index < tokens.len() {
        match &tokens[index] {
            CssToken::Whitespace | CssToken::Comma => {
                index += 1;
            }
            CssToken::Ident(name) if matches!(tokens.get(index + 1), Some(CssToken::ParenOpen)) => {
                let mut depth = 0usize;
                let start = index + 2;
                let mut end = start;
                while end < tokens.len() {
                    match &tokens[end] {
                        CssToken::ParenOpen => depth += 1,
                        CssToken::ParenClose => {
                            if depth == 0 {
                                break;
                            }
                            depth -= 1;
                        }
                        _ => {}
                    }
                    end += 1;
                }

                if end >= tokens.len() {
                    return Err(CssParseError::UnexpectedEndOfInput);
                }

                if name.eq_ignore_ascii_case("url") {
                    values.push(Value::Keyword(format!(
                        "url({})",
                        render_tokens(&tokens[start..end]).trim()
                    )));
                } else {
                    let arguments = if name.eq_ignore_ascii_case("calc") {
                        parse_function_arguments_with_mode(&tokens[start..end], true)?
                    } else {
                        parse_function_arguments_with_mode(
                            &tokens[start..end],
                            preserve_math_delims,
                        )?
                    };
                    values.push(Value::Function {
                        name: name.clone(),
                        arguments,
                    });
                }
                index = end + 1;
            }
            _ => {
                let start = index;
                while index < tokens.len()
                    && !matches!(tokens[index], CssToken::Whitespace | CssToken::Comma)
                {
                    if matches!(
                        tokens.get(index),
                        Some(CssToken::Ident(_)) if matches!(tokens.get(index + 1), Some(CssToken::ParenOpen))
                    ) {
                        break;
                    }
                    if preserve_math_delims && is_math_operator_token(tokens.get(index)) {
                        if index == start {
                            index += 1;
                        }
                        break;
                    }
                    index += 1;
                }
                let segment = &tokens[start..index];
                if segment.is_empty() {
                    continue;
                }
                if segment.len() == 1 {
                    values.push(parse_single_value(&segment[0])?);
                } else {
                    values.push(render_compound_value(segment));
                }
            }
        }
    }

    Ok(values)
}

fn is_math_operator_token(token: Option<&CssToken>) -> bool {
    matches!(
        token,
        Some(CssToken::Delim('+'))
            | Some(CssToken::Delim('-'))
            | Some(CssToken::Delim('*'))
            | Some(CssToken::Delim('/'))
    )
}

fn render_compound_value(tokens: &[CssToken]) -> Value {
    let rendered = render_tokens(tokens);
    if rendered.starts_with('#') {
        Value::Color(rendered)
    } else {
        Value::Keyword(rendered)
    }
}

fn parse_single_value(token: &CssToken) -> Result<Value, CssParseError> {
    match token {
        CssToken::Ident(value) => {
            if value.starts_with('#') {
                Ok(Value::Color(value.clone()))
            } else {
                Ok(Value::Keyword(value.clone()))
            }
        }
        CssToken::Hash(value) => Ok(Value::Color(format!("#{value}"))),
        CssToken::String(value) => Ok(Value::String(value.clone())),
        CssToken::Number(value) => Ok(Value::Number(*value)),
        CssToken::Percentage(value) => Ok(Value::Percentage(*value)),
        CssToken::Dimension(value, unit) => Ok(Value::Length(*value, unit.clone())),
        CssToken::Delim(ch) => Ok(Value::Keyword(ch.to_string())),
        _ => Err(CssParseError::InvalidDeclaration),
    }
}

/// Parses the argument of a `:not()` pseudo-class into a list of simple selectors.
///
/// The argument is a forgiving selector list; only simple selectors (no
/// combinators) are supported here, which is sufficient for CSS Selectors Level 3.
fn parse_not_argument(argument: &str) -> Result<Vec<SimpleSelector>, CssParseError> {
    // Re-tokenize the argument and parse it as simple selectors.
    // Reject if trailing tokens remain (commas, combinators, etc.).
    let tokens = tokenize(argument)?;
    let mut parser = Parser::new(tokens);
    parser.skip_whitespace();
    let selectors = parser.parse_simple_selectors()?;
    parser.skip_whitespace();
    if parser.peek().is_some() {
        return Err(CssParseError::InvalidSelector);
    }
    Ok(selectors)
}

fn split_important(tokens: &[CssToken]) -> (Vec<CssToken>, bool) {
    let compact: Vec<CssToken> = tokens
        .iter()
        .filter(|token| !matches!(token, CssToken::Whitespace))
        .cloned()
        .collect();

    if compact.len() >= 2
        && matches!(compact[compact.len() - 2], CssToken::Delim('!'))
        && matches!(
            &compact[compact.len() - 1],
            CssToken::Ident(keyword) if keyword.eq_ignore_ascii_case("important")
        )
    {
        return (compact[..compact.len() - 2].to_vec(), true);
    }

    (tokens.to_vec(), false)
}

/// Attempt to build a structured `FontFaceRule` from `@font-face` declarations.
///
/// Returns `None` when the required descriptors (`font-family` and a `src` with
/// a `url()`) are missing.
fn build_font_face_rule(declarations: &[Declaration]) -> Option<FontFaceRule> {
    let mut font_family: Option<String> = None;
    let mut src_url: Option<String> = None;
    let mut format: Option<String> = None;
    let mut font_weight: Option<String> = None;
    let mut font_style: Option<String> = None;

    for decl in declarations {
        match decl.name.as_str() {
            "font-family" => {
                font_family = Some(extract_string_value(&decl.value));
            }
            "src" => {
                // Extract url() and optional format() from src value.
                extract_src_descriptor(&decl.value, &mut src_url, &mut format);
            }
            "font-weight" => {
                font_weight = Some(extract_string_value(&decl.value));
            }
            "font-style" => {
                font_style = Some(extract_string_value(&decl.value));
            }
            _ => {}
        }
    }

    let font_family = font_family?;
    let src_url = src_url?;

    Some(FontFaceRule {
        font_family,
        src_url,
        format,
        font_weight,
        font_style,
    })
}

/// Extract a plain string from a CSS value (handles both `String` and `Keyword`).
fn extract_string_value(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Keyword(k) => k.clone(),
        Value::Number(n) => {
            // Format without trailing ".0" for integer-valued floats
            if *n == n.floor() && n.is_finite() {
                format!("{}", *n as i64)
            } else {
                format!("{}", n)
            }
        }
        Value::Percentage(p) => {
            if *p == p.floor() && p.is_finite() {
                format!("{}%", *p as i64)
            } else {
                format!("{}%", p)
            }
        }
        Value::Length(v, unit) => {
            if *v == v.floor() && v.is_finite() {
                format!("{}{}", *v as i64, unit)
            } else {
                format!("{}{}", v, unit)
            }
        }
        Value::List(items) => {
            // font-family can be a list of keywords like `Noto Sans`
            items
                .iter()
                .map(|v| extract_string_value(v))
                .collect::<Vec<_>>()
                .join(" ")
        }
        _ => format!("{:?}", value),
    }
}

/// Parse `src: url(...) format(...)` descriptor.
fn extract_src_descriptor(
    value: &Value,
    out_url: &mut Option<String>,
    out_format: &mut Option<String>,
) {
    match value {
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("url") => {
            if let Some(arg) = arguments.first() {
                *out_url = Some(extract_string_value(arg));
            }
        }
        // The parser produces url() as Keyword("url(...)") — extract the inner URL.
        Value::Keyword(k) if k.starts_with("url(") => {
            *out_url = Some(extract_url_from_keyword(k));
        }
        Value::List(items) => {
            for item in items {
                match item {
                    Value::Function { name, arguments } if name.eq_ignore_ascii_case("url") => {
                        if out_url.is_none() {
                            if let Some(arg) = arguments.first() {
                                *out_url = Some(extract_string_value(arg));
                            }
                        }
                    }
                    Value::Keyword(k) if k.starts_with("url(") => {
                        if out_url.is_none() {
                            *out_url = Some(extract_url_from_keyword(k));
                        }
                    }
                    Value::Function { name, arguments }
                        if name.eq_ignore_ascii_case("format") =>
                    {
                        if let Some(arg) = arguments.first() {
                            *out_format = Some(extract_string_value(arg));
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// Extract the inner URL string from a `"url(...)"` keyword value.
///
/// Handles both `url(foo.ttf)` and `url("foo.ttf")` forms.
fn extract_url_from_keyword(keyword: &str) -> String {
    let inner = keyword
        .strip_prefix("url(")
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or(keyword)
        .trim();
    // Remove surrounding quotes if present
    if (inner.starts_with('"') && inner.ends_with('"'))
        || (inner.starts_with('\'') && inner.ends_with('\''))
    {
        inner[1..inner.len() - 1].to_string()
    } else {
        inner.to_string()
    }
}
