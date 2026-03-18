//! CSS parsing primitives.
//!
//! The CSS layer exposes a tokenizer and a stylesheet parser that later style
//! resolution phases can reuse directly.

use std::fmt;

mod matcher;

pub use matcher::{Specificity, matches_selector, specificity};

/// A token emitted by the CSS tokenizer.
#[derive(Debug, Clone, PartialEq)]
pub enum CssToken {
    Ident(String),
    AtKeyword(String),
    Hash(String),
    String(String),
    Number(f32),
    Percentage(f32),
    Dimension(f32, String),
    Colon,
    Semicolon,
    Comma,
    CurlyOpen,
    CurlyClose,
    ParenOpen,
    ParenClose,
    BracketOpen,
    BracketClose,
    Delim(char),
    Whitespace,
}

/// Selector combinators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Combinator {
    Descendant,
    Child,
    AdjacentSibling,
    GeneralSibling,
}

/// A parsed selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selector {
    pub parts: Vec<SelectorPart>,
}

/// A selector segment plus the combinator to the segment on its left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectorPart {
    pub combinator: Option<Combinator>,
    pub simples: Vec<SimpleSelector>,
}

/// A basic CSS simple selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimpleSelector {
    Type(String),
    Universal,
    Class(String),
    Id(String),
    Attribute {
        name: String,
        operator: Option<AttributeOperator>,
        value: Option<String>,
    },
    PseudoClass(String),
    PseudoElement(String),
}

/// Attribute selector operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributeOperator {
    Equals,
    Includes,
}

/// A stylesheet rule.
#[derive(Debug, Clone, PartialEq)]
pub enum Rule {
    Style(StyleRule),
    At(AtRule),
}

/// A regular style rule.
#[derive(Debug, Clone, PartialEq)]
pub struct StyleRule {
    pub selectors: Vec<Selector>,
    pub declarations: Vec<Declaration>,
}

/// An at-rule.
#[derive(Debug, Clone, PartialEq)]
pub struct AtRule {
    pub name: String,
    pub prelude: String,
    pub block: Option<Vec<Rule>>,
    pub declarations: Vec<Declaration>,
}

/// A CSS declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct Declaration {
    pub name: String,
    pub value: Value,
}

/// CSS values used by declarations.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Keyword(String),
    Length(f32, String),
    Color(String),
    Function { name: String, arguments: Vec<Value> },
    List(Vec<Value>),
    String(String),
    Number(f32),
    Percentage(f32),
}

/// A parsed stylesheet.
#[derive(Debug, Clone, PartialEq)]
pub struct Stylesheet {
    pub rules: Vec<Rule>,
}

/// Errors produced by CSS parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CssParseError {
    UnexpectedEndOfInput,
    ExpectedToken(&'static str),
    InvalidNumber,
    InvalidSelector,
    InvalidDeclaration,
}

impl fmt::Display for CssParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEndOfInput => write!(f, "unexpected end of CSS input"),
            Self::ExpectedToken(token) => write!(f, "expected token: {token}"),
            Self::InvalidNumber => write!(f, "invalid CSS number"),
            Self::InvalidSelector => write!(f, "invalid CSS selector"),
            Self::InvalidDeclaration => write!(f, "invalid CSS declaration"),
        }
    }
}

impl std::error::Error for CssParseError {}

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
            c if is_ident_start(c) || c == '-' => {
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
                    if name == "import" {
                        return Err(CssParseError::InvalidDeclaration);
                    }

                    if name == "media" {
                        let block = self.parse_rule_block()?;
                        return Ok(Rule::At(AtRule {
                            name,
                            prelude: render_tokens(&prelude_tokens).trim().to_string(),
                            block: Some(block),
                            declarations: Vec::new(),
                        }));
                    }

                    let declarations = self.parse_declaration_list()?;
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
            parts.push(SelectorPart { combinator, simples });
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
                            while !matches!(self.peek(), Some(CssToken::ParenClose) | None) {
                                argument_tokens.push(self.next().expect("peeked token should exist"));
                            }
                            match self.next() {
                                Some(CssToken::ParenClose) => {}
                                _ => return Err(CssParseError::ExpectedToken(")")),
                            }
                            let argument = render_tokens(&argument_tokens).trim().to_string();
                            SimpleSelector::PseudoClass(format!("{name}({argument})"))
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

        let value = parse_value_tokens(&value_tokens)?;
        Ok(expand_shorthand(&name, value))
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

    let values = parse_value_sequence(&tokens)?;
    if values.len() == 1 {
        Ok(values.into_iter().next().expect("single item must exist"))
    } else {
        Ok(Value::List(values))
    }
}

fn parse_function_arguments(tokens: &[CssToken]) -> Result<Vec<Value>, CssParseError> {
    let mut arguments = Vec::new();
    let mut segment = Vec::new();
    for token in tokens {
        if matches!(token, CssToken::Comma) {
            if !segment.is_empty() {
                arguments.push(parse_value_tokens(&segment)?);
                segment.clear();
            }
            continue;
        }
        segment.push(token.clone());
    }
    if !segment.is_empty() {
        arguments.push(parse_value_tokens(&segment)?);
    }
    Ok(arguments)
}

fn parse_value_sequence(tokens: &[CssToken]) -> Result<Vec<Value>, CssParseError> {
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

                let arguments = parse_function_arguments(&tokens[start..end])?;
                values.push(Value::Function {
                    name: name.clone(),
                    arguments,
                });
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
        _ => Err(CssParseError::InvalidDeclaration),
    }
}

fn expand_shorthand(name: &str, value: Value) -> Vec<Declaration> {
    match name {
        "margin" | "padding" => expand_box_shorthand(name, value),
        "border" => expand_border_shorthand(value),
        _ => vec![Declaration {
            name: name.to_string(),
            value,
        }],
    }
}

fn expand_box_shorthand(prefix: &str, value: Value) -> Vec<Declaration> {
    let values = match value {
        Value::List(values) => values,
        single => vec![single],
    };

    let (top, right, bottom, left) = match values.as_slice() {
        [a] => (a.clone(), a.clone(), a.clone(), a.clone()),
        [a, b] => (a.clone(), b.clone(), a.clone(), b.clone()),
        [a, b, c] => (a.clone(), b.clone(), c.clone(), b.clone()),
        [a, b, c, d] => (a.clone(), b.clone(), c.clone(), d.clone()),
        _ => return vec![Declaration {
            name: prefix.to_string(),
            value: Value::List(values),
        }],
    };

    vec![
        Declaration {
            name: format!("{prefix}-top"),
            value: top,
        },
        Declaration {
            name: format!("{prefix}-right"),
            value: right,
        },
        Declaration {
            name: format!("{prefix}-bottom"),
            value: bottom,
        },
        Declaration {
            name: format!("{prefix}-left"),
            value: left,
        },
    ]
}

fn expand_border_shorthand(value: Value) -> Vec<Declaration> {
    let values = match value {
        Value::List(values) => values,
        single => vec![single],
    };

    let mut width = None;
    let mut style = None;
    let mut color = None;

    for item in values {
        let is_width_keyword = matches!(
            &item,
            Value::Keyword(keyword) if matches!(keyword.as_str(), "thin" | "medium" | "thick")
        );
        let is_border_style = matches!(
            &item,
            Value::Keyword(keyword)
                if matches!(
                    keyword.as_str(),
                    "none"
                        | "hidden"
                        | "dotted"
                        | "dashed"
                        | "solid"
                        | "double"
                        | "groove"
                        | "ridge"
                        | "inset"
                        | "outset"
                )
        );

        match item {
            Value::Length(_, _) if width.is_none() => width = Some(item),
            Value::Keyword(_) if is_width_keyword && width.is_none() => width = Some(item),
            Value::Keyword(_) if is_border_style && style.is_none() => style = Some(item),
            Value::Color(_) | Value::Function { .. } | Value::Keyword(_) if color.is_none() => {
                color = Some(item)
            }
            _ => {}
        }
    }

    let mut declarations = Vec::new();
    if let Some(width) = width {
        declarations.push(Declaration {
            name: "border-width".to_string(),
            value: width,
        });
    }
    if let Some(style) = style {
        declarations.push(Declaration {
            name: "border-style".to_string(),
            value: style,
        });
    }
    if let Some(color) = color {
        declarations.push(Declaration {
            name: "border-color".to_string(),
            value: color,
        });
    }

    if declarations.is_empty() {
        declarations.push(Declaration {
            name: "border".to_string(),
            value: Value::List(Vec::new()),
        });
    }

    declarations
}

fn render_tokens(tokens: &[CssToken]) -> String {
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

fn trimmed_number(value: f32) -> String {
    let mut text = value.to_string();
    if text.ends_with(".0") {
        text.truncate(text.len() - 2);
    }
    text
}

fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_' || ch == '-'
}

fn is_ident_char(ch: char) -> bool {
    is_ident_start(ch) || ch.is_ascii_digit()
}

fn is_number_start(chars: &[char], index: usize) -> bool {
    chars.get(index).is_some_and(|c| c.is_ascii_digit())
        || (chars.get(index) == Some(&'.') && chars.get(index + 1).is_some_and(|c| c.is_ascii_digit()))
}

fn consume_ident(chars: &[char], index: &mut usize) -> String {
    let mut ident = String::new();
    while let Some(&ch) = chars.get(*index) {
        if is_ident_char(ch) {
            ident.push(ch);
            *index += 1;
        } else {
            break;
        }
    }
    ident
}

fn consume_string(chars: &[char], index: &mut usize, quote: char) -> Result<String, CssParseError> {
    *index += 1;
    let mut value = String::new();
    while let Some(&ch) = chars.get(*index) {
        *index += 1;
        if ch == quote {
            return Ok(value);
        }
        value.push(ch);
    }
    Err(CssParseError::UnexpectedEndOfInput)
}

fn consume_number(chars: &[char], index: &mut usize) -> Result<f32, CssParseError> {
    let mut value = String::new();
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
    value.parse::<f32>().map_err(|_| CssParseError::InvalidNumber)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_basic_css() {
        let tokens = tokenize("h1 { color: red; margin: 10px; }").unwrap();
        assert!(tokens.contains(&CssToken::Ident("h1".to_string())));
        assert!(tokens.contains(&CssToken::Dimension(10.0, "px".to_string())));
        assert!(tokens.contains(&CssToken::CurlyOpen));
    }

    #[test]
    fn parses_style_rule_with_selectors() {
        let stylesheet = parse_stylesheet("div.hero > #title, a:hover { color: #fff; }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };

        assert_eq!(rule.selectors.len(), 2);
        assert_eq!(rule.selectors[0].parts[0].simples[0], SimpleSelector::Type("div".to_string()));
        assert_eq!(rule.selectors[0].parts[0].simples[1], SimpleSelector::Class("hero".to_string()));
        assert_eq!(rule.selectors[0].parts[1].combinator, Some(Combinator::Child));
        assert_eq!(rule.selectors[1].parts[0].simples[0], SimpleSelector::Type("a".to_string()));
        assert_eq!(rule.selectors[1].parts[0].simples[1], SimpleSelector::PseudoClass("hover".to_string()));
    }

    #[test]
    fn parses_attribute_and_pseudo_element_selectors() {
        let stylesheet = parse_stylesheet(r#"input[type="text"]::placeholder { color: gray; }"#).unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };

        assert_eq!(
            rule.selectors[0].parts[0].simples,
            vec![
                SimpleSelector::Type("input".to_string()),
                SimpleSelector::Attribute {
                    name: "type".to_string(),
                    operator: Some(AttributeOperator::Equals),
                    value: Some("text".to_string()),
                },
                SimpleSelector::PseudoElement("placeholder".to_string()),
            ]
        );
    }

    #[test]
    fn parses_values_and_functions() {
        let stylesheet = parse_stylesheet("body { width: calc(100%); background-color: rgb(255, 0, 0); }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };

        assert_eq!(
            rule.declarations[0].value,
            Value::Function {
                name: "calc".to_string(),
                arguments: vec![Value::Percentage(100.0)],
            }
        );
        assert_eq!(
            rule.declarations[1].value,
            Value::Function {
                name: "rgb".to_string(),
                arguments: vec![Value::Number(255.0), Value::Number(0.0), Value::Number(0.0)],
            }
        );
    }

    #[test]
    fn parses_at_rules() {
        let stylesheet = parse_stylesheet(
            r#"@import "base.css"; @font-face { font-family: "Demo"; src: url(font.woff2); } @media screen { h1 { color: blue; } }"#,
        )
        .unwrap();

        assert_eq!(stylesheet.rules.len(), 3);
        let Rule::At(import_rule) = &stylesheet.rules[0] else {
            panic!("expected import rule");
        };
        assert_eq!(import_rule.name, "import");
        assert_eq!(import_rule.prelude, "\"base.css\"");

        let Rule::At(font_face) = &stylesheet.rules[1] else {
            panic!("expected font-face rule");
        };
        assert_eq!(font_face.declarations.len(), 2);

        let Rule::At(media_rule) = &stylesheet.rules[2] else {
            panic!("expected media rule");
        };
        assert!(media_rule.block.is_some());
    }

    #[test]
    fn expands_margin_and_border_shorthands() {
        let stylesheet = parse_stylesheet("div { margin: 1px 2px; border: 1px solid #000; }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };

        assert!(rule.declarations.iter().any(|decl| decl.name == "margin-top"));
        assert!(rule.declarations.iter().any(|decl| decl.name == "margin-right"));
        assert!(rule.declarations.iter().any(|decl| decl.name == "border-width"));
        assert!(rule.declarations.iter().any(|decl| decl.name == "border-style"));
        assert!(rule.declarations.iter().any(|decl| decl.name == "border-color"));
    }
}
