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

/// Parses a standalone selector list, requiring the complete input to be valid.
pub fn parse_selector_list(input: &str) -> Result<Vec<Selector>, CssParseError> {
    let tokens = tokenize(input)?;
    let mut parser = Parser::new(tokens);
    parser.skip_whitespace();
    if parser.peek().is_none() {
        return Err(CssParseError::InvalidSelector);
    }
    let selectors = parser.parse_selector_list_until(false)?;
    parser.skip_whitespace();
    if parser.peek().is_some() {
        return Err(CssParseError::InvalidSelector);
    }
    if !selectors.iter().all(selector_is_supported_for_dom_query) {
        return Err(CssParseError::InvalidSelector);
    }
    Ok(selectors)
}

/// Parses an inline `style` attribute as a forgiving declaration list.
///
/// Invalid declarations are skipped independently. If tokenization of the
/// complete input fails, independently tokenizable top-level declarations are
/// still retained.
pub fn parse_style_attribute(input: &str) -> Vec<Declaration> {
    match tokenize(input) {
        Ok(tokens) => Parser::new(tokens).parse_style_attribute(),
        Err(_) => split_style_attribute_chunks(input)
            .into_iter()
            .filter_map(|chunk| tokenize(chunk).ok())
            .flat_map(|tokens| Parser::new(tokens).parse_style_attribute())
            .collect(),
    }
}

fn split_style_attribute_chunks(input: &str) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut start = 0usize;
    let mut quote = None;
    let mut escaped = false;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;

    for (index, ch) in input.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active_quote {
                quote = None;
            }
            continue;
        }

        match ch {
            '\'' | '"' => quote = Some(ch),
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            ';' if paren_depth == 0 && bracket_depth == 0 => {
                chunks.push(&input[start..index]);
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    chunks.push(&input[start..]);
    chunks
}

fn selector_is_supported_for_dom_query(selector: &Selector) -> bool {
    selector.parts.iter().all(|part| {
        part.simples.iter().all(|simple| match simple {
            SimpleSelector::PseudoClass(name) => {
                // Pseudo-class names are ASCII case-insensitive
                // (`:NTH-CHILD(odd)` is valid), so normalize before gating.
                let base = name.split_once('(').map_or(name.as_str(), |(base, _)| base);
                let base = base.to_ascii_lowercase();
                let known = matches!(
                    base.as_str(),
                    "root" | "first-child" | "last-child" | "only-child"
                        | "nth-child" | "nth-last-child" | "first-of-type"
                        | "last-of-type" | "only-of-type" | "nth-of-type"
                        | "nth-last-of-type" | "empty" | "lang" | "enabled"
                        | "disabled" | "checked" | "before" | "after"
                );
                known
                    && (!base.starts_with("nth-")
                        || name
                            .split_once('(')
                            .and_then(|(_, rest)| rest.strip_suffix(')'))
                            .and_then(super::matcher::parse_an_plus_b)
                            .is_some())
            }
            SimpleSelector::PseudoElement(name) => {
                name.eq_ignore_ascii_case("before") || name.eq_ignore_ascii_case("after")
            }
            SimpleSelector::Not(inner) => inner.iter().all(|simple| {
                selector_is_supported_for_dom_query(&Selector {
                    parts: vec![SelectorPart {
                        combinator: None,
                        simples: vec![simple.clone()],
                    }],
                })
            }),
            _ => true,
        })
    })
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

                    if name.eq_ignore_ascii_case("media")
                        || name.eq_ignore_ascii_case("layer")
                    {
                        let block = self.parse_rule_block()?;
                        return Ok(Rule::At(AtRule {
                            name,
                            prelude: render_tokens(&prelude_tokens).trim().to_string(),
                            block: Some(block),
                            declarations: Vec::new(),
                        }));
                    }

                    // @keyframes: skip the block entirely and store as raw text
                    // because keyframe selectors (0%, 100%, from, to) are not
                    // valid CSS selectors and would fail parse_rule_block().
                    if name.eq_ignore_ascii_case("keyframes")
                        || name.eq_ignore_ascii_case("-webkit-keyframes")
                    {
                        let raw_block = self.consume_raw_block()?;
                        return Ok(Rule::At(AtRule {
                            name,
                            prelude: render_tokens(&prelude_tokens).trim().to_string(),
                            block: None,
                            // Store raw block text in a single declaration for later parsing.
                            declarations: vec![Declaration {
                                name: "__keyframes_block".to_string(),
                                value: Value::Keyword(raw_block),
                                important: false,
                            }],
                        }));
                    }

                    let declarations = self.parse_declaration_list()?;

                    // Produce a structured FontFace rule when possible.
                    if name.eq_ignore_ascii_case("font-face")
                        && let Some(ff) = build_font_face_rule(&declarations) {
                            return Ok(Rule::FontFace(ff));
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

    /// Consumes tokens until the matching closing brace, returning the raw text.
    /// The opening brace has already been consumed.
    fn consume_raw_block(&mut self) -> Result<String, CssParseError> {
        let mut depth = 1;
        let mut text = String::new();
        while let Some(token) = self.next() {
            match &token {
                CssToken::CurlyOpen => {
                    depth += 1;
                    text.push('{');
                }
                CssToken::CurlyClose => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(text);
                    }
                    text.push('}');
                }
                _ => {
                    text.push_str(&render_tokens(&[token]));
                }
            }
        }
        Err(CssParseError::UnexpectedEndOfInput)
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
        let selectors = self.parse_selector_list_until(true)?;
        self.expect_curly_open()?;
        let declarations = self.parse_declaration_list()?;
        Ok(Rule::Style(StyleRule {
            selectors,
            declarations,
        }))
    }

    fn parse_selector_list_until(
        &mut self,
        allow_curly_open: bool,
    ) -> Result<Vec<Selector>, CssParseError> {
        let mut selectors = Vec::new();
        loop {
            selectors.push(self.parse_selector()?);
            self.skip_whitespace();
            match self.peek() {
                Some(CssToken::Comma) => {
                    self.next();
                    self.skip_whitespace();
                    if self.peek().is_none()
                        || (allow_curly_open && matches!(self.peek(), Some(CssToken::CurlyOpen)))
                        || matches!(self.peek(), Some(CssToken::Comma))
                    {
                        return Err(CssParseError::InvalidSelector);
                    }
                }
                Some(CssToken::CurlyOpen) if allow_curly_open => break,
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
                    if parts.is_empty() || combinator.is_some() {
                        return Err(CssParseError::InvalidSelector);
                    }
                    self.next();
                    combinator = Some(Combinator::Child);
                    continue;
                }
                Some(CssToken::Delim('+')) => {
                    if parts.is_empty() || combinator.is_some() {
                        return Err(CssParseError::InvalidSelector);
                    }
                    self.next();
                    combinator = Some(Combinator::AdjacentSibling);
                    continue;
                }
                Some(CssToken::Delim('~')) => {
                    if parts.is_empty() || combinator.is_some() {
                        return Err(CssParseError::InvalidSelector);
                    }
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

        if parts.is_empty() || combinator.is_some() {
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
                    simples.push(self.parse_attribute_selector()?);
                }
                Some(CssToken::Colon) => {
                    simples.push(self.parse_pseudo_selector()?);
                }
                _ => break,
            }
        }

        if simples.is_empty() {
            return Err(CssParseError::InvalidSelector);
        }

        let type_selector_count = simples
            .iter()
            .filter(|simple| matches!(simple, SimpleSelector::Type(_) | SimpleSelector::Universal))
            .count();
        if type_selector_count > 1 {
            return Err(CssParseError::InvalidSelector);
        }

        Ok(simples)
    }

    /// Parses `[attr op "value"]` attribute selectors.
    /// The opening `[` is consumed here.
    fn parse_attribute_selector(&mut self) -> Result<SimpleSelector, CssParseError> {
        self.next(); // consume BracketOpen
        self.skip_whitespace();
        let name = self.expect_ident()?;
        self.skip_whitespace();
        let operator = self.parse_attribute_operator()?;
        self.skip_whitespace();
        let value = if operator.is_some() {
            Some(self.expect_ident_or_string()?)
        } else {
            None
        };
        self.skip_whitespace();
        self.expect_bracket_close()?;
        Ok(SimpleSelector::Attribute {
            name,
            operator,
            value,
        })
    }

    /// Parses the operator inside an attribute selector (e.g. `=`, `~=`, `^=`).
    /// Returns `None` when no operator is present (presence-only selector).
    fn parse_attribute_operator(&mut self) -> Result<Option<AttributeOperator>, CssParseError> {
        match self.peek() {
            Some(CssToken::Delim('=')) => {
                self.next();
                Ok(Some(AttributeOperator::Equals))
            }
            Some(CssToken::Delim('~')) => {
                self.next();
                self.expect_delim('=')?;
                Ok(Some(AttributeOperator::Includes))
            }
            Some(CssToken::Delim('^')) => {
                self.next();
                self.expect_delim('=')?;
                Ok(Some(AttributeOperator::StartsWith))
            }
            Some(CssToken::Delim('$')) => {
                self.next();
                self.expect_delim('=')?;
                Ok(Some(AttributeOperator::EndsWith))
            }
            Some(CssToken::Delim('*')) => {
                self.next();
                self.expect_delim('=')?;
                Ok(Some(AttributeOperator::Contains))
            }
            Some(CssToken::Delim('|')) => {
                self.next();
                self.expect_delim('=')?;
                Ok(Some(AttributeOperator::DashMatch))
            }
            _ => Ok(None),
        }
    }

    /// Parses `:pseudo-class` or `::pseudo-element` selectors.
    /// The leading `:` is consumed here.
    fn parse_pseudo_selector(&mut self) -> Result<SimpleSelector, CssParseError> {
        self.next(); // consume first Colon
        if matches!(self.peek(), Some(CssToken::Colon)) {
            self.next();
            let name = self.expect_ident()?;
            if !matches!(self.peek(), Some(CssToken::ParenOpen)) {
                return Ok(SimpleSelector::PseudoElement(name));
            }
            if !name.eq_ignore_ascii_case("slotted") {
                return Err(CssParseError::InvalidSelector);
            }
            self.next(); // consume ParenOpen
            let argument = render_tokens(&self.collect_parenthesized_tokens()?)
                .trim()
                .to_string();
            if argument.is_empty() {
                return Err(CssParseError::InvalidSelector);
            }
            return Ok(SimpleSelector::PseudoElement(format!("{name}({argument})")));
        }
        let name = self.expect_ident()?;
        if !matches!(self.peek(), Some(CssToken::ParenOpen)) {
            return Ok(SimpleSelector::PseudoClass(name));
        }
        self.next(); // consume ParenOpen
        let argument_tokens = self.collect_parenthesized_tokens()?;
        if name == "not" {
            let argument_str = render_tokens(&argument_tokens).trim().to_string();
            let inner = parse_not_argument(&argument_str)?;
            Ok(SimpleSelector::Not(inner))
        } else {
            let argument = if name.to_ascii_lowercase().starts_with("nth-") {
                render_nth_argument(&argument_tokens)
            } else {
                render_tokens(&argument_tokens).trim().to_string()
            };
            if argument.is_empty() {
                return Err(CssParseError::InvalidSelector);
            }
            Ok(SimpleSelector::PseudoClass(format!("{name}({argument})")))
        }
    }

    /// Collects tokens inside a parenthesized group, tracking nesting depth.
    /// The opening `(` must already be consumed. Consumes the closing `)`.
    fn collect_parenthesized_tokens(&mut self) -> Result<Vec<CssToken>, CssParseError> {
        let mut tokens = Vec::new();
        let mut depth = 0usize;
        loop {
            match self.peek() {
                Some(CssToken::ParenOpen) => {
                    depth += 1;
                    tokens.push(self.next().expect("peeked token should exist"));
                }
                Some(CssToken::ParenClose) if depth > 0 => {
                    depth -= 1;
                    tokens.push(self.next().expect("peeked token should exist"));
                }
                Some(CssToken::ParenClose) | None => break,
                _ => {
                    tokens.push(self.next().expect("peeked token should exist"));
                }
            }
        }
        match self.next() {
            Some(CssToken::ParenClose) => {}
            _ => return Err(CssParseError::ExpectedToken(")")),
        }
        Ok(tokens)
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

    fn parse_style_attribute(&mut self) -> Vec<Declaration> {
        let mut declarations = Vec::new();
        loop {
            self.skip_whitespace();
            while matches!(self.peek(), Some(CssToken::Semicolon)) {
                self.next();
                self.skip_whitespace();
            }
            if self.peek().is_none() {
                break;
            }

            match self.parse_declaration() {
                Ok(mut parsed) => declarations.append(&mut parsed),
                Err(_) => self.skip_invalid_declaration(),
            }

            if matches!(self.peek(), Some(CssToken::Semicolon)) {
                self.next();
            }
        }
        declarations
    }

    fn skip_invalid_declaration(&mut self) {
        let mut paren_depth = 0usize;
        let mut bracket_depth = 0usize;
        while let Some(token) = self.peek() {
            match token {
                CssToken::Semicolon if paren_depth == 0 && bracket_depth == 0 => break,
                CssToken::ParenOpen => paren_depth += 1,
                CssToken::ParenClose => paren_depth = paren_depth.saturating_sub(1),
                CssToken::BracketOpen => bracket_depth += 1,
                CssToken::BracketClose => bracket_depth = bracket_depth.saturating_sub(1),
                _ => {}
            }
            self.next();
        }
    }

    fn parse_declaration(&mut self) -> Result<Vec<Declaration>, CssParseError> {
        let name = self.expect_ident()?.to_ascii_lowercase();
        self.skip_whitespace();
        self.expect_colon()?;
        self.skip_whitespace();

        let mut value_tokens = Vec::new();
        let mut paren_depth = 0usize;
        let mut bracket_depth = 0usize;
        while let Some(token) = self.peek() {
            match token {
                CssToken::Semicolon if paren_depth == 0 && bracket_depth == 0 => break,
                CssToken::CurlyClose => break,
                CssToken::ParenOpen => {
                    paren_depth += 1;
                    value_tokens.push(self.next().expect("peeked token should exist"));
                }
                CssToken::ParenClose => {
                    paren_depth = paren_depth.saturating_sub(1);
                    value_tokens.push(self.next().expect("peeked token should exist"));
                }
                CssToken::BracketOpen => {
                    bracket_depth += 1;
                    value_tokens.push(self.next().expect("peeked token should exist"));
                }
                CssToken::BracketClose => {
                    bracket_depth = bracket_depth.saturating_sub(1);
                    value_tokens.push(self.next().expect("peeked token should exist"));
                }
                _ => value_tokens.push(self.next().expect("peeked token should exist")),
            }
        }

        let (value_tokens, important) = split_important(&value_tokens);
        // CSS masking is intentionally single-layer for now. Preserve the first
        // top-level layer and ignore subsequent comma-separated layers before
        // generic value parsing (which otherwise discards comma tokens).
        let value_tokens = if matches!(
            name.as_str(),
            "mask"
                | "-webkit-mask"
                | "mask-image"
                | "-webkit-mask-image"
                | "mask-position"
                | "-webkit-mask-position"
                | "mask-size"
                | "-webkit-mask-size"
                | "mask-repeat"
                | "-webkit-mask-repeat"
        ) {
            first_mask_layer(&value_tokens)
        } else {
            value_tokens
        };
        let value = if matches!(name.as_str(), "mask" | "-webkit-mask") {
            // Preserve `/` even when authors omit surrounding whitespace, as
            // in the common `top center/contain` form.
            parse_value_tokens_with_mode(&value_tokens, true)?
        } else {
            parse_value_tokens(&value_tokens)?
        };
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

fn first_mask_layer(tokens: &[CssToken]) -> Vec<CssToken> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        match token {
            CssToken::ParenOpen | CssToken::BracketOpen => depth += 1,
            CssToken::ParenClose | CssToken::BracketClose => depth = depth.saturating_sub(1),
            CssToken::Comma if depth == 0 => return tokens[..index].to_vec(),
            _ => {}
        }
    }
    tokens.to_vec()
}

fn render_nth_argument(tokens: &[CssToken]) -> String {
    let mut rendered = String::new();
    for (index, token) in tokens.iter().enumerate() {
        let piece = render_tokens(std::slice::from_ref(token));
        // The tokenizer folds a '+' that directly precedes digits into the
        // number itself (`2n+3` tokenizes as `2n`, `3`), so the sign has to be
        // re-inserted when rendering. Only do so when the number *immediately*
        // follows the `n` token: with intervening whitespace (`2n 3`) the
        // source had no operator at all and must stay invalid. Known
        // limitation: `2n +3` (space before a folded `+`) is tokenized
        // identically to `2n 3` and is therefore also rejected — write
        // `2n + 3` or `2n+3` instead. A negative b keeps its sign in the
        // token value, so `2n -3` stays valid.
        let follows_n_directly = index > 0
            && !matches!(tokens[index - 1], CssToken::Whitespace)
            && rendered.ends_with(['n', 'N']);
        if matches!(token, CssToken::Number(value) | CssToken::Dimension(value, _) if *value >= 0.0)
            && follows_n_directly
        {
            rendered.push('+');
        }
        rendered.push_str(&piece);
    }
    rendered.trim().to_string()
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
    let Some(important_index) = tokens
        .iter()
        .rposition(|token| !matches!(token, CssToken::Whitespace))
    else {
        return (Vec::new(), false);
    };
    if matches!(
        &tokens[important_index],
        CssToken::Ident(keyword) if keyword.eq_ignore_ascii_case("important")
    ) {
        let bang_index = tokens[..important_index]
            .iter()
            .rposition(|token| !matches!(token, CssToken::Whitespace));
        if let Some(bang_index) = bang_index
            && matches!(tokens[bang_index], CssToken::Delim('!'))
            && token_is_at_top_level(tokens, bang_index)
        {
            let mut value_end = bang_index;
            while value_end > 0 && matches!(tokens[value_end - 1], CssToken::Whitespace) {
                value_end -= 1;
            }
            return (tokens[..value_end].to_vec(), true);
        }
    }

    (tokens.to_vec(), false)
}

fn token_is_at_top_level(tokens: &[CssToken], index: usize) -> bool {
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    for token in &tokens[..index] {
        match token {
            CssToken::ParenOpen => paren_depth += 1,
            CssToken::ParenClose => paren_depth = paren_depth.saturating_sub(1),
            CssToken::BracketOpen => bracket_depth += 1,
            CssToken::BracketClose => bracket_depth = bracket_depth.saturating_sub(1),
            _ => {}
        }
    }
    paren_depth == 0 && bracket_depth == 0
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
                .map(extract_string_value)
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
                        if let Some(arg) = arguments.first() {
                            *out_url = Some(extract_string_value(arg));
                            *out_format = None;
                        }
                    }
                    Value::Keyword(k) if k.starts_with("url(") => {
                        *out_url = Some(extract_url_from_keyword(k));
                        *out_format = None;
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

#[cfg(test)]
mod selector_list_tests {
    use super::*;

    #[test]
    fn standalone_selector_list_accepts_lists_and_function_arguments() {
        let selectors = parse_selector_list(
            "div.foo, #bar > input:nth-child( -n + 3 ):nth-of-type(3n-1)",
        )
        .unwrap();
        assert_eq!(selectors.len(), 2);
        assert!(format!("{:?}", selectors).contains("nth-child(-n + 3)"));
        assert!(format!("{:?}", selectors).contains("nth-of-type(3n-1)"));

        for selector in [":lang(en)", ":nth-last-of-type(-5n+3)"] {
            assert_eq!(parse_selector_list(selector).unwrap().len(), 1);
        }
    }

    #[test]
    fn standalone_selector_list_rejects_empty_or_partial_lists() {
        for invalid in ["", "   ", ",div", "div,", "div,,span"] {
            assert!(parse_selector_list(invalid).is_err(), "accepted {invalid:?}");
        }
    }

    #[test]
    fn standalone_selector_list_rejects_an_plus_b_missing_operator() {
        // A whitespace-separated b with no operator is not valid an+b
        // grammar and must not be silently normalized into `an+b`.
        for invalid in [
            ":nth-child(2n 3)",
            ":nth-child(n 3)",
            ":nth-last-of-type(2n 1)",
        ] {
            assert!(parse_selector_list(invalid).is_err(), "accepted {invalid:?}");
        }
        // Whitespace around an explicit operator stays valid, and a negative
        // b keeps its sign inside the number token.
        for valid in [
            ":nth-child(2n + 3)",
            ":nth-child(2n+3)",
            ":nth-child(2n - 1)",
            ":nth-child(2n -1)",
        ] {
            assert!(parse_selector_list(valid).is_ok(), "rejected {valid:?}");
        }
    }

    #[test]
    fn standalone_selector_list_accepts_uppercase_pseudo_names() {
        for valid in [":NTH-CHILD(odd)", ":First-Child", "::BEFORE", ":LANG(en)"] {
            assert!(parse_selector_list(valid).is_ok(), "rejected {valid:?}");
        }
    }

    #[test]
    fn stylesheet_only_accepts_slotted_as_functional_pseudo_element() {
        for valid in ["::slotted(.item)", "::SLOTTED(*)"] {
            let stylesheet = parse_stylesheet(&format!("{valid} {{ color: red; }}")).unwrap();
            assert_eq!(stylesheet.rules.len(), 1, "rejected {valid:?}");
        }
        for invalid in ["::before(foo)", "::after(.item)", "::part(name)"] {
            assert!(
                parse_stylesheet(&format!("{invalid} {{ color: red; }}")).is_err(),
                "accepted {invalid:?}"
            );
        }
    }

    #[test]
    fn standalone_selector_list_rejects_invalid_structure() {
        for invalid in [
            "> div",
            "div >",
            "div > + span",
            "div:lang()",
            "div:nth-child(2n+1",
            "html*.test",
            "div { color: red }",
        ] {
            assert!(parse_selector_list(invalid).is_err(), "accepted {invalid:?}");
        }
    }
}
